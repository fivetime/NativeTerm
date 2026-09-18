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
use crate::folder_options;
use crate::header;
use crate::options;
use crate::persistent;
use crate::plink::{self, PlinkSession};
use crate::securecrt::{self, Plan};
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
    /// Typed after every login (`NativeTermOnLogin`), one line.
    pub on_login: Option<String>,
    /// The host's own `NativeTermPersistent` (`tmux`, `screen`, `off`);
    /// `None` follows the folder.
    pub persistent: Option<String>,
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
            on_login: host.nt.get("onlogin").map(str::to_string),
            persistent: host.nt.get(persistent::KEY).map(str::to_string),
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
        if self.on_login.as_deref().is_some_and(|n| n.contains(['\r', '\n'])) {
            return Err(EditError::Invalid("the login command must be one line".into()));
        }
        if let Some(p) = &self.persistent {
            if !matches!(p.as_str(), "tmux" | "screen" | "off") {
                return Err(EditError::Invalid(format!("persistent session {p:?}: tmux, screen or off")));
            }
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

/// ssh-only operations refuse non-SSH sessions (they have their own).
fn not_plink(host: &HostEntry) -> Result<(), EditError> {
    match host.plink {
        Some(_) => Err(EditError::Invalid(format!("{} is not an ssh host", host.alias()))),
        None => Ok(()),
    }
}

fn named<'s>(sessions: &'s mut [PlinkSession], alias: &str) -> Result<&'s mut PlinkSession, EditError> {
    sessions
        .iter_mut()
        .find(|s| s.name.eq_ignore_ascii_case(alias))
        .ok_or_else(|| EditError::NotFound(format!("session {alias}")))
}

pub struct Editor {
    ssh_dir: PathBuf,
    writer: Writer,
    ssh: PathBuf,
    /// `None`: ssh's own default (`~/.ssh/config`). For tests: an explicit
    /// main config, whose includes must then be absolute.
    config: Option<PathBuf>,
    /// The `Include` line NativeTerm maintains in the main config.
    include: String,
    /// Where folder files live (`config.d` unless moved).
    folders: PathBuf,
}

/// A folder's file: its ssh hosts (`.conf`) or its non-SSH sessions
/// (`.nt.toml`).
fn is_folder_file(path: &Path) -> bool {
    let name = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    name.ends_with(".conf") || name.ends_with(&format!(".{}", plink::EXTENSION))
}

/// A folder's files in `dir` (`.conf` and `.nt.toml`), sorted.
fn folder_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|entries| entries.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.is_file() && is_folder_file(p)).collect())
        .unwrap_or_default();
    files.sort();
    files
}

/// Whether `dir` holds session folders already (synced from another
/// computer, for example).
pub fn holds_folders(dir: &Path) -> bool {
    !folder_files(dir).is_empty()
}

/// The `Include` pattern for folder files in `dir`.
fn include_for(dir: &Path) -> String {
    format!("{}/*.conf", dir.to_string_lossy().replace('\\', "/"))
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
            folders: ssh_dir.join("config.d"),
        }
    }

    /// For another directory (tests): ssh is pointed at its config with
    /// `-F`, and the include is absolute.
    pub fn for_directory(ssh_dir: &Path, writer: Writer, ssh: &Path) -> Editor {
        Editor {
            ssh_dir: ssh_dir.to_path_buf(),
            writer,
            ssh: ssh.to_path_buf(),
            config: Some(ssh_dir.join("config")),
            include: include_for(&ssh_dir.join("config.d")),
            folders: ssh_dir.join("config.d"),
        }
    }

    /// Folder files live in `dir` (a per-machine setting); the `Include`
    /// line then names it absolutely.
    pub fn with_folders_dir(mut self, dir: &Path) -> Editor {
        self.include = include_for(dir);
        self.folders = dir.to_path_buf();
        self
    }

    pub fn main_config(&self) -> PathBuf {
        self.ssh_dir.join("config")
    }

    pub fn folders_dir(&self) -> PathBuf {
        self.folders.clone()
    }

    /// Move the folder files to `new_dir` and point the main config's
    /// `Include` at it. The files are written like any edit (owner-only
    /// access), ssh must accept the result and see the same hosts, or
    /// everything is undone. The old folder is left as it was; ssh no
    /// longer reads it. Returns how many files were moved.
    pub fn move_folders(&mut self, new_dir: &Path) -> Result<usize, EditError> {
        let norm = |p: &Path| p.to_string_lossy().trim_end_matches(['\\', '/']).replace('/', "\\").to_lowercase();
        if !new_dir.is_absolute() {
            return Err(EditError::Invalid("not a full path".into()));
        }
        if norm(new_dir) == norm(&self.folders) {
            return Err(EditError::Invalid("that is where the folders are".into()));
        }
        let conf = folder_files;
        if !conf(new_dir).is_empty() {
            return Err(EditError::Invalid(format!("{} already holds session folders", new_dir.display())));
        }
        let before: std::collections::BTreeSet<String> =
            SessionTree::load(&self.ssh_dir).hosts().map(|(_, h)| h.alias().to_lowercase()).collect();
        std::fs::create_dir_all(new_dir)?;
        let files = conf(&self.folders);
        let mut written = Vec::new();
        let undo = |written: &[PathBuf]| {
            for f in written {
                let _ = std::fs::remove_file(f);
            }
        };
        for file in &files {
            let target = new_dir.join(file.file_name().unwrap_or_default());
            let copied = write::read(file).map_err(EditError::from).and_then(|(text, _)| {
                self.writer.write(&target, &text, None, || Ok(())).map(|_| ()).map_err(EditError::from)
            });
            if let Err(e) = copied {
                undo(&written);
                return Err(e);
            }
            written.push(target);
        }
        let (old, new) = (self.include.clone(), include_for(new_dir));
        let ssh_dir = self.ssh_dir.clone();
        let result = edit_file(
            &self.writer,
            &self.main_config(),
            |doc| {
                header::replace_include(doc, &old, &new);
            },
            || {
                self.validate(PARSE_CHECK_HOST, None)?;
                let after: std::collections::BTreeSet<String> =
                    SessionTree::load(&ssh_dir).hosts().map(|(_, h)| h.alias().to_lowercase()).collect();
                if after != before {
                    return Err(format!("{} hosts before, {} after the move", before.len(), after.len()));
                }
                Ok(())
            },
        );
        if let Err(e) = result {
            undo(&written);
            return Err(e.into());
        }
        self.include = new;
        self.folders = new_dir.to_path_buf();
        Ok(files.len())
    }

    /// Add a non-SSH session to the folder of `folder_file` (a `.conf` or
    /// the main config); its name must be free among all sessions.
    pub fn add_plink(&self, folder_file: &Path, session: &PlinkSession) -> Result<(), EditError> {
        session.check().map_err(EditError::Invalid)?;
        if SessionTree::load(&self.ssh_dir).taken_aliases().contains(&session.name.to_ascii_lowercase()) {
            return Err(EditError::Invalid(format!("{} is taken", session.name)));
        }
        self.edit_plink(&plink::sibling(folder_file), |sessions| {
            sessions.push(session.clone());
            Ok(())
        })
    }

    /// Replace a non-SSH session (a new name must be free).
    pub fn update_plink(&self, host: &HostEntry, session: &PlinkSession) -> Result<(), EditError> {
        if host.plink.is_none() {
            return Err(EditError::Invalid(format!("{} is an ssh host", host.alias())));
        }
        session.check().map_err(EditError::Invalid)?;
        let alias = host.alias().to_string();
        if !session.name.eq_ignore_ascii_case(&alias)
            && SessionTree::load(&self.ssh_dir).taken_aliases().contains(&session.name.to_ascii_lowercase())
        {
            return Err(EditError::Invalid(format!("{} is taken", session.name)));
        }
        self.edit_plink(&host.file, |sessions| {
            *named(sessions, &alias)? = session.clone();
            Ok(())
        })
    }

    /// Change a `.nt.toml` through the writer (backup, conflict check);
    /// every session must still be valid.
    fn edit_plink(
        &self,
        file: &Path,
        change: impl FnOnce(&mut Vec<PlinkSession>) -> Result<(), EditError>,
    ) -> Result<(), EditError> {
        let (text, fingerprint) = write::read(file)?;
        let mut sessions = plink::parse(&text).map_err(|e| EditError::Invalid(format!("{}: {e}", file.display())))?;
        change(&mut sessions)?;
        for s in &sessions {
            s.check().map_err(EditError::Invalid)?;
        }
        self.writer.write(file, &plink::render(&sessions), fingerprint, || Ok(()))?;
        Ok(())
    }

    /// Use the session folders already in `dir` instead of the current
    /// ones (another computer's, synced): only the main config's
    /// `Include` changes, nothing is copied, and ssh must accept the
    /// result, or it is undone. The current folder is left as it was.
    /// Returns how many folder files `dir` has.
    pub fn adopt_folders(&mut self, dir: &Path) -> Result<usize, EditError> {
        if !dir.is_absolute() {
            return Err(EditError::Invalid("not a full path".into()));
        }
        let files = folder_files(dir);
        if files.is_empty() {
            return Err(EditError::Invalid(format!("{} holds no session folders", dir.display())));
        }
        let (old, new) = (self.include.clone(), include_for(dir));
        edit_file(
            &self.writer,
            &self.main_config(),
            |doc| {
                header::replace_include(doc, &old, &new);
            },
            || self.validate(PARSE_CHECK_HOST, None),
        )?;
        self.include = new;
        self.folders = dir.to_path_buf();
        Ok(files.len())
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

    /// Persistent sessions for every host in a folder file that has no
    /// setting of its own (`tmux`, `screen`; `None` removes the default).
    pub fn set_folder_persistent(&self, file: &Path, value: Option<&str>) -> Result<(), EditError> {
        if value.is_some_and(|v| !matches!(v, "tmux" | "screen")) {
            return Err(EditError::Invalid(format!("persistent sessions {value:?}: tmux or screen")));
        }
        if file == self.main_config() {
            return Err(EditError::Invalid("the main config has no folder settings".into()));
        }
        let stem = file.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        edit_file(
            &self.writer,
            file,
            |doc| {
                let block = match doc.find_host_block(FOLDER_DEFAULTS_HOST) {
                    Some(block) => block,
                    None if value.is_none() => return,
                    None => {
                        prepend_folder_block(doc, &stem);
                        let Some(block) = doc.find_host_block(FOLDER_DEFAULTS_HOST) else { return };
                        block
                    }
                };
                set_or_remove(doc, block, "NativeTermPersistent", value);
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
            |doc| {
                doc.append_host(&[&new_alias], &refs);
                folder_options::arrange(doc, &folder_options::tag_for(file));
            },
            || self.validate(&new_alias, Some(draft.hostname.trim())),
        )?;
        Ok(new_alias)
    }

    /// Change a host's settings in place.
    pub fn update_host(&self, host: &HostEntry, draft: &HostDraft) -> Result<(), EditError> {
        not_plink(host)?;
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
                set_or_remove(doc, block, "NativeTermOnLogin", draft.on_login.as_deref().filter(|n| !n.trim().is_empty()));
                set_or_remove(doc, block, "NativeTermPersistent", draft.persistent.as_deref());
            },
            || self.validate(&alias, Some(draft.hostname.trim())),
        )?;
        Ok(())
    }

    /// The session options written in a host's own block.
    pub fn host_options(&self, host: &HostEntry) -> Result<options::Values, EditError> {
        not_plink(host)?;
        let (text, _) = write::read(&host.file)?;
        let doc = Document::parse(&text);
        match doc.find_host_block(host.alias()) {
            Some(block) => Ok(options::read(&doc, block)),
            None => Err(EditError::NotFound(format!("host {} in {}", host.alias(), host.file.display()))),
        }
    }

    /// Write session options into a host's block; `ssh -G` must accept
    /// them, or the file is rolled back with ssh's message.
    pub fn set_host_options(&self, host: &HostEntry, values: &options::Values) -> Result<(), EditError> {
        not_plink(host)?;
        let values = options::normalize(values).map_err(EditError::Invalid)?;
        self.host_options(host)?;
        let alias = host.alias().to_string();
        edit_file(
            &self.writer,
            &host.file,
            |doc| {
                if let Some(block) = doc.find_host_block(&alias) {
                    options::apply(doc, block, &values);
                }
            },
            || self.validate(&alias, host.hostname.as_deref()),
        )?;
        Ok(())
    }

    /// Whether this ssh can apply folder options (`Tag`, OpenSSH 9.4+).
    pub fn folder_options_supported(&self) -> bool {
        let mut command = std::process::Command::new(&self.ssh);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        command
            .arg("-V")
            .stdin(std::process::Stdio::null())
            .output()
            .is_ok_and(|o| folder_options::supported(&String::from_utf8_lossy(&o.stderr)))
    }

    /// The options for every host in a folder file.
    pub fn folder_options(&self, file: &Path) -> Result<options::Values, EditError> {
        let (text, _) = write::read(file)?;
        Ok(folder_options::read(&Document::parse(&text), &folder_options::tag_for(file)))
    }

    /// Set a folder's options; `ssh -G` must accept the file (checked with
    /// one of its hosts, which the options apply to). Returns hosts that
    /// keep a `Tag` of their own and so don't get the options.
    pub fn set_folder_options(&self, file: &Path, values: &options::Values) -> Result<Vec<String>, EditError> {
        if file == self.main_config() {
            return Err(EditError::Invalid("folder options are for folder files, not the main config".into()));
        }
        let values = options::normalize(values).map_err(EditError::Invalid)?;
        let tag = folder_options::tag_for(file);
        let (text, _) = write::read(file)?;
        let mut preview = Document::parse(&text);
        let own = folder_options::apply(&mut preview, &tag, &values);
        let probe = SessionTree::load(&self.ssh_dir)
            .folders()
            .find(|f| f.file == file)
            .and_then(|f| f.hosts.first().map(|h| h.alias().to_string()));
        edit_file(
            &self.writer,
            file,
            |doc| {
                folder_options::apply(doc, &tag, &values);
            },
            || match &probe {
                Some(alias) => self.validate(alias, None),
                None => self.validate(PARSE_CHECK_HOST, None),
            },
        )?;
        Ok(own)
    }

    /// What ssh uses for a host (`ssh -G`): lowercase keyword and value,
    /// repeated keywords once per value.
    pub fn effective(&self, alias: &str) -> Result<Vec<(String, String)>, String> {
        effective::effective_with(&self.ssh, self.config.as_deref(), alias).map_err(|e| e.to_string())
    }

    pub fn ssh(&self) -> &Path {
        &self.ssh
    }

    /// For asking ssh about many hosts off the UI thread.
    pub fn checker(&self) -> effective::Checker {
        effective::Checker { ssh: self.ssh.clone(), config: self.config.clone() }
    }

    /// Mark or unmark a host as a favorite (`NativeTermFavorite yes`).
    pub fn set_favorite(&self, host: &HostEntry, on: bool) -> Result<(), EditError> {
        let alias = host.alias().to_string();
        if host.plink.is_some() {
            return self.edit_plink(&host.file, |sessions| {
                named(sessions, &alias)?.favorite = on;
                Ok(())
            });
        }
        edit_file(
            &self.writer,
            &host.file,
            |doc| {
                if let Some(block) = doc.find_host_block(&alias) {
                    set_or_remove(doc, block, "NativeTermFavorite", on.then_some("yes"));
                }
            },
            || self.validate(&alias, host.hostname.as_deref()),
        )?;
        Ok(())
    }

    pub fn delete_host(&self, host: &HostEntry) -> Result<(), EditError> {
        let alias = host.alias().to_string();
        if host.plink.is_some() {
            return self.edit_plink(&host.file, |sessions| {
                named(sessions, &alias)?;
                sessions.retain(|s| !s.name.eq_ignore_ascii_case(&alias));
                Ok(())
            });
        }
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
        if let Some(session) = &host.plink {
            let target = plink::sibling(to);
            if host.file == target {
                return Ok(());
            }
            self.edit_plink(&target, |sessions| {
                sessions.push((**session).clone());
                Ok(())
            })?;
            let alias = host.alias().to_string();
            let removed = self.edit_plink(&host.file, |sessions| {
                named(sessions, &alias)?;
                sessions.retain(|s| !s.name.eq_ignore_ascii_case(&alias));
                Ok(())
            });
            if let Err(e) = removed {
                // take the copy out again, so it isn't defined twice
                let _ = self.edit_plink(&target, |sessions| {
                    sessions.retain(|s| !s.name.eq_ignore_ascii_case(&alias));
                    Ok(())
                });
                return Err(e);
            }
            return Ok(());
        }
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
        let target_tag = folder_options::tag_for(to);
        edit_file(
            &self.writer,
            to,
            |doc| {
                append_raw(doc, &lines);
                if let Some(b) = last_host_block(doc, &alias) {
                    folder_options::untag(doc, b);
                }
                folder_options::arrange(doc, &target_tag);
            },
            || Ok(()),
        )?;
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

/// Result of [`Editor::import`]: each folder is written whole or not at all.
#[derive(Debug, Default)]
pub struct ImportOutcome {
    /// Folder label, file, sessions written (ssh and non-SSH).
    pub written: Vec<(String, PathBuf, usize)>,
    /// Folder label and why it wasn't written.
    pub failed: Vec<(String, String)>,
    /// Host keys added to `known_hosts`.
    pub keys_added: usize,
    /// Why `known_hosts` wasn't changed, if it failed.
    pub keys_failed: Option<String>,
}

impl ImportOutcome {
    pub fn hosts(&self) -> usize {
        self.written.iter().map(|(_, _, n)| n).sum()
    }
}

/// Folders written and checked at the same time during an import.
const IMPORT_WORKERS: usize = 8;

impl Editor {
    /// Write an import plan (see [`securecrt::plan`]). One write per folder,
    /// validated by resolving every new alias with `ssh -G`; a folder that
    /// fails is rolled back and reported, the others are kept. `progress`
    /// gets (hosts done, hosts total) after each folder.
    ///
    /// Folders are written in parallel. While one folder's file is in place
    /// unchecked, it could make another folder's check fail (ssh reads all
    /// of them), so failed folders are tried once more on their own.
    pub fn import(&self, plan: &Plan, progress: &(dyn Fn(usize, usize) + Sync)) -> Result<ImportOutcome, EditError> {
        securecrt::check_unique(plan).map_err(EditError::Invalid)?;
        let total = plan.host_count();
        let mut keys = ImportOutcome::default();
        match self.add_host_keys(&plan.host_keys) {
            Ok(n) => keys.keys_added = n,
            Err(e) => keys.keys_failed = Some(e.to_string()),
        }
        if total == 0 {
            return Ok(keys);
        }
        self.ensure_header()?;
        let dir = self.folders_dir();
        std::fs::create_dir_all(&dir)?;

        // file names up front, so parallel writers don't pick the same one
        let mut used = std::collections::HashSet::new();
        let jobs: Vec<(&securecrt::PlannedFolder, PathBuf)> = plan
            .folders
            .iter()
            .filter(|f| f.session_count() > 0)
            .map(|folder| {
                let path = folder.existing.clone().unwrap_or_else(|| {
                    let stem = securecrt::folder_stem(&folder.label);
                    (1..)
                        .map(|n| if n == 1 { dir.join(format!("{stem}.conf")) } else { dir.join(format!("{stem}-{n}.conf")) })
                        .find(|p| !p.exists() && !plink::sibling(p).exists() && !used.contains(p))
                        .expect("unbounded")
                });
                used.insert(path.clone());
                (folder, path)
            })
            .collect();

        let done = std::sync::atomic::AtomicUsize::new(0);
        let next = std::sync::atomic::AtomicUsize::new(0);
        let results: std::sync::Mutex<Vec<Option<Result<(), String>>>> = std::sync::Mutex::new(vec![None; jobs.len()]);
        let run = |i: usize| {
            let (folder, path) = &jobs[i];
            let result = self.write_folder(folder, path);
            if result.is_ok() {
                let count = folder.session_count();
                let n = done.fetch_add(count, std::sync::atomic::Ordering::SeqCst) + count;
                progress(n, total);
            }
            results.lock().unwrap_or_else(|e| e.into_inner())[i] = Some(result);
        };
        std::thread::scope(|scope| {
            for _ in 0..IMPORT_WORKERS.min(jobs.len()) {
                scope.spawn(|| loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if i >= jobs.len() {
                        break;
                    }
                    run(i);
                });
            }
        });
        let failed: Vec<usize> = results
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .enumerate()
            .filter(|(_, r)| !matches!(r, Some(Ok(()))))
            .map(|(i, _)| i)
            .collect();
        for i in failed {
            run(i);
        }

        let mut outcome = keys;
        let results = results.into_inner().unwrap_or_else(|e| e.into_inner());
        for ((folder, path), result) in jobs.iter().zip(results) {
            match result {
                Some(Ok(())) => outcome.written.push((folder.label.clone(), path.clone(), folder.session_count())),
                Some(Err(e)) => outcome.failed.push((folder.label.clone(), e)),
                None => outcome.failed.push((folder.label.clone(), "not attempted".into())),
            }
        }
        Ok(outcome)
    }

    /// The names `known_hosts` may list `host` under: its host name and its
    /// alias, with the port when it isn't 22.
    pub fn host_key_names(host: &crate::HostEntry) -> Vec<String> {
        let port = host.port.unwrap_or(22);
        let mut names = vec![crate::known_hosts::host_name(host.target(), port)];
        let alias = crate::known_hosts::host_name(host.alias(), port);
        if !names.contains(&alias) {
            names.push(alias);
        }
        names
    }

    /// Forget the host keys of `host` (after a reinstall, for example).
    pub fn forget_host_keys(&self, host: &crate::HostEntry) -> std::io::Result<Vec<String>> {
        let keygen = crate::known_hosts::ssh_keygen_for(&self.ssh);
        crate::known_hosts::remove(&keygen, &self.known_hosts(), &Editor::host_key_names(host))
    }

    /// `~/.ssh/known_hosts` (created owner-only if missing).
    pub fn known_hosts(&self) -> PathBuf {
        self.ssh_dir.join("known_hosts")
    }

    /// Add the keys `known_hosts` doesn't have yet; returns how many.
    pub fn add_host_keys(&self, keys: &[crate::known_hosts::HostKey]) -> Result<usize, EditError> {
        if keys.is_empty() {
            return Ok(0);
        }
        let path = self.known_hosts();
        let (text, fingerprint) = match write::read(&path) {
            Ok(read) => read,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (String::new(), None),
            Err(e) => return Err(e.into()),
        };
        let missing = crate::known_hosts::missing(&text, keys);
        if missing.is_empty() {
            return Ok(0);
        }
        let mut new = text.clone();
        if !new.is_empty() && !new.ends_with('\n') {
            new.push('\n');
        }
        for key in &missing {
            new.push_str(&key.line());
            new.push('\n');
        }
        self.writer.write(&path, &new, fingerprint, || Ok(()))?;
        Ok(missing.len())
    }

    /// Append one planned folder's sessions: non-SSH ones to the
    /// `.nt.toml` beside `path` first, then the ssh hosts to `path`
    /// (created if new, even for non-SSH sessions only: it makes the
    /// folder), each alias checked. If the hosts can't be written, the
    /// non-SSH sessions are taken out again, so a retry starts clean.
    fn write_folder(&self, folder: &securecrt::PlannedFolder, path: &Path) -> Result<(), String> {
        let sessions_file = plink::sibling(path);
        if !folder.plink.is_empty() {
            self.edit_plink(&sessions_file, |sessions| {
                sessions.extend(folder.plink.iter().cloned());
                Ok(())
            })
            .map_err(|e| e.to_string())?;
        }
        let written = self.write_folder_hosts(folder, path);
        if written.is_err() && !folder.plink.is_empty() {
            let names: std::collections::HashSet<&str> = folder.plink.iter().map(|s| s.name.as_str()).collect();
            let _ = self.edit_plink(&sessions_file, |sessions| {
                sessions.retain(|s| !names.contains(s.name.as_str()));
                Ok(())
            });
            if folder.existing.is_none() && plink::read(&sessions_file).is_ok_and(|s| s.is_empty()) {
                let _ = std::fs::remove_file(&sessions_file);
            }
        }
        written
    }

    fn write_folder_hosts(&self, folder: &securecrt::PlannedFolder, path: &Path) -> Result<(), String> {
        let (text, fingerprint) = match &folder.existing {
            Some(_) => write::read(path).map_err(|e| e.to_string())?,
            None => {
                if path.exists() {
                    // an earlier attempt of this import left nothing behind;
                    // anything there now belongs to someone else
                    return Err(format!("{} appeared meanwhile", path.display()));
                }
                let mut doc = Document::parse("");
                doc.append_host(&[FOLDER_DEFAULTS_HOST], &[("NativeTermLabel", folder.label.as_str())]);
                (doc.render(), None)
            }
        };
        let mut doc = Document::parse(&text);
        for host in &folder.hosts {
            let mut lines = vec![format!("Host {}", crate::document::quote_arg(&host.alias))];
            lines.extend(
                host.entries(&crate::new_id())
                    .into_iter()
                    .map(|(keyword, value)| format!("    {keyword} {}", directive_args(keyword, &value))),
            );
            append_raw(&mut doc, &lines);
        }
        folder_options::arrange(&mut doc, &folder_options::tag_for(path));
        let checks = || {
            folder.hosts.iter().try_for_each(|h| {
                self.validate(&h.alias, Some(&h.hostname)).map_err(|e| format!("{}: {e}", h.alias))
            })
        };
        self.writer.write(path, &doc.render(), fingerprint, checks).map(|_| ()).map_err(|e| e.to_string())
    }
}

/// Forwards take two arguments; everything else one.
fn directive_args(keyword: &str, value: &str) -> String {
    match keyword {
        "LocalForward" | "RemoteForward" => {
            value.split_whitespace().map(crate::document::quote_arg).collect::<Vec<_>>().join(" ")
        }
        _ => crate::document::quote_arg(value),
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
    if let Some(command) = draft.on_login.as_deref().filter(|n| !n.trim().is_empty()) {
        entries.push(("NativeTermOnLogin", command.to_string()));
    }
    if let Some(persistent) = &draft.persistent {
        entries.push(("NativeTermPersistent", persistent.clone()));
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

    fn telnet(name: &str, host: &str) -> PlinkSession {
        PlinkSession { name: name.into(), host: Some(host.into()), ..PlinkSession::default() }
    }

    /// Non-SSH sessions: added, favorited, renamed, moved and deleted in
    /// the folders' `.nt.toml`; names are shared with ssh's aliases; ssh
    /// operations refuse them and ssh never sees them.
    #[test]
    fn plink_sessions_are_edited() {
        let Some((_home, editor)) = setup() else { return };
        let prod = editor.create_folder("生产").unwrap();
        let lab = editor.create_folder("Lab").unwrap();
        editor.add_plink(&prod, &telnet("sw", "10.0.0.5")).unwrap();
        assert!(editor.add_plink(&prod, &telnet("old", "10.0.0.6")).is_err(), "an ssh host's name");
        assert!(editor.add_plink(&prod, &telnet("SW", "10.0.0.6")).is_err(), "taken, any case");
        assert!(editor.add_plink(&prod, &telnet("bad name", "h")).is_err());
        assert!(prod.with_file_name("shengchan.nt.toml").exists());
        let conf = std::fs::read_to_string(&prod).unwrap();
        assert!(!conf.contains("sw"), "the .conf is untouched: {conf}");

        let host = |name: &str| tree(&editor).find(name).map(|(_, h)| h.clone()).unwrap();
        editor.set_favorite(&host("sw"), true).unwrap();
        assert!(host("sw").favorite());
        assert!(editor.update_host(&host("sw"), &draft("x", "h")).is_err(), "not through the ssh dialog");
        assert!(editor.host_options(&host("sw")).is_err());

        let mut renamed = host("sw").plink.unwrap().as_ref().clone();
        renamed.name = "core-sw".into();
        renamed.label = Some("核心交换机".into());
        assert!(editor.update_plink(&host("sw"), &PlinkSession { name: "old".into(), ..renamed.clone() }).is_err());
        editor.update_plink(&host("sw"), &renamed).unwrap();
        assert_eq!(host("core-sw").label(), "核心交换机");
        assert!(host("core-sw").favorite(), "kept");

        editor.move_host(&host("core-sw"), &lab).unwrap();
        let moved = host("core-sw");
        assert_eq!(moved.file, lab.with_file_name("lab.nt.toml"));
        assert_eq!(tree(&editor).folders().find(|f| f.file == prod).unwrap().hosts.len(), 0);

        editor.delete_host(&moved).unwrap();
        assert!(tree(&editor).find("core-sw").is_none());
        assert!(editor.delete_host(&moved).is_err(), "gone already");
        // ssh itself is unaffected throughout
        assert!(editor.effective("old").is_ok());
    }

    fn draft(label: &str, hostname: &str) -> HostDraft {
        HostDraft { label: label.into(), hostname: hostname.into(), ..HostDraft::default() }
    }

    /// A second computer: its sessions are in the synced folder already,
    /// so it points its `Include` there instead of copying.
    #[test]
    fn synced_folders_are_adopted() {
        let Some((home, mut editor)) = setup() else { return };
        let synced = home.path().join("OneDrive").join("NativeTerm");
        assert!(editor.adopt_folders(&synced).is_err(), "nothing there yet");
        std::fs::create_dir_all(&synced).unwrap();
        std::fs::write(synced.join("prod.conf"), "Host web\n    HostName 10.0.0.1\n").unwrap();
        crate::acl::restrict_to_owner(&synced.join("prod.conf")).unwrap();
        std::fs::write(synced.join("prod.nt.toml"), "[[session]]\nname = \"sw\"\nhost = \"10.0.0.2\"\n").unwrap();
        assert!(holds_folders(&synced));
        assert!(editor.move_folders(&synced).is_err(), "moving onto them is refused");
        assert_eq!(editor.adopt_folders(&synced).unwrap(), 2);
        assert_eq!(editor.folders_dir(), synced);
        let t = tree(&editor);
        assert!(t.find("web").is_some() && t.find("sw").is_some());
        assert!(editor.effective("web").unwrap().iter().any(|(k, v)| k == "hostname" && v == "10.0.0.1"));
    }

    #[test]
    fn folders_move_elsewhere() {
        let Some((home, mut editor)) = setup() else { return };
        let prod = editor.create_folder("生产").unwrap();
        editor.create_host(&tree(&editor), &prod, &draft("web", "10.0.0.1")).unwrap();
        editor.add_plink(&prod, &telnet("sw", "10.0.0.5")).unwrap();
        editor.create_folder("Lab").unwrap();
        let old_dir = editor.folders_dir();
        let hosts = |e: &Editor| tree(e).hosts().map(|(_, h)| h.alias().to_string()).collect::<Vec<_>>();
        let before = hosts(&editor);

        assert!(editor.move_folders(Path::new("relative")).is_err());
        assert!(editor.move_folders(&old_dir).is_err());
        let busy = home.path().join("busy");
        std::fs::create_dir_all(&busy).unwrap();
        std::fs::write(busy.join("x.conf"), "").unwrap();
        assert!(editor.move_folders(&busy).is_err(), "a folder with .conf files");

        let new_dir = home.path().join("synced folders");
        assert_eq!(editor.move_folders(&new_dir).unwrap(), 3, "two folders, one with non-SSH sessions");
        assert_eq!(editor.folders_dir(), new_dir);
        assert_eq!(hosts(&editor), before);
        assert!(tree(&editor).find("sw").unwrap().1.file.starts_with(&new_dir));
        let main = std::fs::read_to_string(editor.main_config()).unwrap();
        assert!(main.contains("synced folders/*.conf\""), "{main}");
        assert!(!main.contains("config.d/*.conf"), "{main}");
        assert!(old_dir.join("shengchan.conf").exists(), "the old folder is left alone");
        // editing works in the new place, and ssh sees it
        let t = tree(&editor);
        let file = t.folders().find(|f| f.label() == "生产").unwrap().file.clone();
        assert!(file.starts_with(&new_dir));
        editor.create_host(&t, &file, &draft("db", "10.0.0.2")).unwrap();
        assert!(editor.effective("db").unwrap().iter().any(|(k, v)| k == "hostname" && v == "10.0.0.2"));
        let lab = editor.create_folder("Ops").unwrap();
        assert!(lab.starts_with(&new_dir));
    }

    #[test]
    fn folder_options_reach_its_hosts() {
        let Some((_home, editor)) = setup() else { return };
        if !editor.folder_options_supported() {
            eprintln!("skipped: ssh older than 9.4");
            return;
        }
        let setting = |alias: &str, key: &str| {
            editor.effective(alias).unwrap().into_iter().find(|(k, _)| k == key).map(|(_, v)| v).unwrap_or_default()
        };
        let prod = editor.create_folder("生产").unwrap();
        let lab = editor.create_folder("Lab").unwrap();
        let web = editor.create_host(&tree(&editor), &prod, &draft("web", "10.0.0.1")).unwrap();
        let mut own = draft("db", "10.0.0.2");
        own.user = Some("dba".into());
        let db = editor.create_host(&tree(&editor), &prod, &own).unwrap();

        let mut values = options::empty();
        values.insert("User", vec!["ops".into()]);
        values.insert("ServerAliveInterval", vec!["15".into()]);
        assert!(editor.set_folder_options(&prod, &values).unwrap().is_empty());
        assert_eq!(editor.folder_options(&prod).unwrap(), values);
        assert_eq!(setting(&web, "user"), "ops");
        assert_eq!(setting(&web, "serveraliveinterval"), "15");
        assert_eq!(setting(&db, "user"), "dba", "the host's own value wins");
        assert_eq!(setting("old", "serveraliveinterval"), "0", "other files aren't touched");

        // a host created afterwards gets them too; one moved out loses them
        let api = editor.create_host(&tree(&editor), &prod, &draft("api", "10.0.0.3")).unwrap();
        assert_eq!(setting(&api, "user"), "ops");
        let text = std::fs::read_to_string(&prod).unwrap();
        assert!(text.trim_end().ends_with("Match tagged nativeterm-shengchan\n    ServerAliveInterval 15\n    User ops"), "options stay last: {text}");
        let t = tree(&editor);
        editor.move_host(t.find(&web).unwrap().1, &lab).unwrap();
        assert_eq!(setting(&web, "serveraliveinterval"), "0");
        assert!(!std::fs::read_to_string(&lab).unwrap().contains("Tag"));
        // moved back: tagged again, the block still last
        let t = tree(&editor);
        editor.move_host(t.find(&web).unwrap().1, &prod).unwrap();
        assert_eq!(setting(&web, "user"), "ops");

        // ssh refuses a bad value: nothing changes
        let before = std::fs::read_to_string(&prod).unwrap();
        values.insert("Ciphers", vec!["no-such-cipher".into()]);
        assert!(editor.set_folder_options(&prod, &values).is_err());
        assert_eq!(std::fs::read_to_string(&prod).unwrap(), before);
        assert!(editor.set_folder_options(&editor.main_config(), &values).is_err());

        // cleared: no block, no tags
        editor.set_folder_options(&prod, &options::empty()).unwrap();
        let text = std::fs::read_to_string(&prod).unwrap();
        assert!(!text.contains("Match") && !text.contains("Tag"), "{text}");
        // back to ssh's default: the local user name, like any other host
        assert_eq!(setting(&api, "user"), setting("old", "user"));
    }

    #[test]
    fn folders_and_hosts_round_trip() {
        let Some((_home, editor)) = setup() else { return };
        let folder = editor.create_folder("Ceph 集群").unwrap();
        assert_eq!(folder.file_name().unwrap(), "ceph-jiqun.conf", "{}", folder.display());
        let main = std::fs::read_to_string(editor.main_config()).unwrap();
        assert!(main.contains("IgnoreUnknown NativeTerm*") && main.contains("Include "), "{main}");
        assert!(main.contains("Host old"), "own content kept: {main}");

        let mut d = draft("osd 1 (生产)", "10.32.16.70");
        d.user = Some("ops".into());
        d.port = Some(2222);
        d.identity_files = vec!["~/.ssh/id_ed25519".into()];
        d.note = Some("rack 3".into());
        d.on_login = Some("cd /srv && sudo -i".into());
        let alias = editor.create_host(&tree(&editor), &folder, &d).unwrap();
        assert_eq!(alias, "osd-1-shengchan");

        let t = tree(&editor);
        let (f, host) = t.find(&alias).unwrap();
        assert_eq!(f.label(), "Ceph 集群");
        assert_eq!(HostDraft::from_host(host), d);
        assert!(host.id().is_some());

        // same label again: the folder prefix keeps the alias unique
        let second = editor.create_host(&t, &folder, &draft("osd 1 (生产)", "10.32.16.71")).unwrap();
        assert_eq!(second, "ceph-jiqun.osd-1-shengchan");

        // edit: drop the port and the note, rename, second key
        let mut changed = d.clone();
        changed.label = "osd-1-shengchan".into();
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
        assert!(tree(&editor).find("ceph-jiqun.osd-1-shengchan").is_some());

        let t = tree(&editor);
        let host = t.find("ceph-jiqun.osd-1-shengchan").unwrap().1.clone();
        let mut values = editor.host_options(&host).unwrap();
        values.insert("Ciphers", vec!["+aes128-cbc".into()]);
        values.insert("LocalForward", vec!["8080 localhost:80".into(), "8443 localhost:443".into()]);
        editor.set_host_options(&host, &values).unwrap();
        assert_eq!(editor.host_options(&host).unwrap(), values);
        let effective = editor.effective(host.alias()).unwrap();
        assert!(effective.iter().any(|(k, v)| k == "ciphers" && v.contains("aes128-cbc")), "{effective:?}");
        assert_eq!(effective.iter().filter(|(k, _)| k == "localforward").count(), 2);
        values.insert("ForwardAgent", vec!["yes".into()]);
        editor.set_host_options(&host, &values).unwrap();
        let aliases = vec![host.alias().to_string(), "old".to_string(), "-bad".to_string()];
        assert_eq!(editor.checker().forwarding_agent(&aliases), [host.alias()]);
        values.insert("ForwardAgent", Vec::new());
        // ssh refuses a bad cipher: nothing changes
        let before = std::fs::read_to_string(&host.file).unwrap();
        values.insert("Ciphers", vec!["no-such-cipher".into()]);
        let error = editor.set_host_options(&host, &values).unwrap_err().to_string();
        assert!(error.to_lowercase().contains("cipher"), "{error}");
        assert_eq!(std::fs::read_to_string(&host.file).unwrap(), before);

        editor.set_favorite(&host, true).unwrap();
        assert_eq!(tree(&editor).find("ceph-jiqun.osd-1-shengchan").unwrap().1.nt.get("favorite"), Some("yes"));
        editor.set_favorite(&host, false).unwrap();
        assert_eq!(tree(&editor).find("ceph-jiqun.osd-1-shengchan").unwrap().1.nt.get("favorite"), None);

        editor.rename_folder(&folder, "Ceph").unwrap();
        assert_eq!(tree(&editor).folders().find(|f| f.file == folder).unwrap().label(), "Ceph");
    }

    /// `NativeTermPersistent`: a folder default reaches its hosts, a host's
    /// own value (including `off`) wins, and ssh accepts the file.
    #[test]
    fn persistent_sessions_per_folder_and_host() {
        use crate::persistent::{for_host, Persistence};
        let Some((_home, editor)) = setup() else { return };
        let prod = editor.create_folder("生产").unwrap();
        editor.create_host(&tree(&editor), &prod, &draft("web", "10.0.0.1")).unwrap();
        let mut own = draft("db", "10.0.0.2");
        own.persistent = Some("off".into());
        editor.create_host(&tree(&editor), &prod, &own).unwrap();
        let setting = |alias: &str| {
            let t = tree(&editor);
            let (folder, host) = t.find(alias).unwrap();
            for_host(folder, host)
        };
        assert_eq!(setting("web"), None);

        editor.set_folder_persistent(&prod, Some("tmux")).unwrap();
        assert_eq!(setting("web"), Some(Persistence::Tmux), "the folder default");
        assert_eq!(setting("db"), None, "the host's own off wins");
        assert_eq!(tree(&editor).find("web").unwrap().0.label(), "生产", "the folder keeps its name");
        assert!(editor.effective("web").is_ok(), "ssh ignores the key");

        let web = tree(&editor).find("web").unwrap().1.clone();
        let mut screen = HostDraft::from_host(&web);
        screen.persistent = Some("screen".into());
        editor.update_host(&web, &screen).unwrap();
        assert_eq!(setting("web"), Some(Persistence::Screen));
        screen.persistent = Some("mosh".into());
        assert!(editor.update_host(&web, &screen).is_err());

        editor.set_folder_persistent(&prod, None).unwrap();
        assert_eq!(setting("web"), Some(Persistence::Screen), "its own value stays");
        assert!(!std::fs::read_to_string(&prod).unwrap().contains("NativeTermPersistent tmux"));
        assert!(editor.set_folder_persistent(&editor.main_config(), Some("tmux")).is_err());
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

    fn crt_file(dir: &Path, rel: &str, body: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, format!("\u{feff}S:\"Protocol Name\"=SSH2\r\n{body}")).unwrap();
    }

    #[test]
    fn imports_a_plan() {
        let Some((home, editor)) = setup() else { return };
        let config = home.path().join("crt");
        let crt = config.join("Sessions");
        crt_file(&crt, "生产/bastion.ini", "S:\"Hostname\"=10.1.0.1\r\nS:\"Username\"=ops\r\n");
        crt_file(
            &crt,
            "生产/web 01.ini",
            concat!(
                "S:\"Hostname\"=10.1.0.2\r\n",
                "D:\"[SSH2] Port\"=00000d3d\r\n",
                "S:\"Firewall Name\"=Session:生产/bastion\r\n",
                "Z:\"Port Forward Table V2\"=00000002\r\n",
                " web|127.0.0.1,8080|1|10.0.0.9|80||\r\n",
                " dyn|1080|0|socks,|0||\r\n",
                "Z:\"Description\"=00000001\r\n",
                " 前端 web\r\n",
            ),
        );
        crt_file(&crt, "old-box.ini", "S:\"Hostname\"=10.9.9.9\r\n");
        let scan = securecrt::scan(&config).unwrap();
        let plan = securecrt::plan(&scan, &tree(&editor));
        assert_eq!(plan.host_count(), 3);
        assert!(plan.host_keys.is_empty(), "no KnownHosts folder here");
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let outcome = editor
            .import(&plan, &|_, _| {
                calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            })
            .unwrap();
        assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);
        assert_eq!(outcome.hosts(), 3);
        assert_eq!(calls.into_inner(), 2);

        let t = tree(&editor);
        let (folder, web) = t.find("web-01").unwrap();
        assert_eq!(folder.label(), "生产");
        assert_eq!(web.label(), "web 01");
        assert_eq!(web.port, Some(3389));
        assert_eq!(web.proxy_jump.as_deref(), Some("bastion"));
        assert_eq!(web.nt.get("note"), Some("前端 web"));
        assert_eq!(web.nt.get("source"), Some("securecrt:生产/web 01"));
        let effective = crate::effective::effective_with(&editor.ssh, editor.config.as_deref(), "web-01").unwrap();
        let get = |k: &str| effective.iter().filter(|(kk, _)| kk == k).map(|(_, v)| v.clone()).collect::<Vec<_>>();
        assert_eq!(get("localforward"), ["[127.0.0.1]:8080 [10.0.0.9]:80"]);
        assert_eq!(get("dynamicforward"), ["1080"]);
        assert_eq!(t.find("old-box").unwrap().0.label(), "SecureCRT");

        // a second import skips what is there and adds the new session to
        // the existing folder
        crt_file(&crt, "生产/db.ini", "S:\"Hostname\"=10.1.0.3\r\n");
        let scan = securecrt::scan(&config).unwrap();
        let plan = securecrt::plan(&scan, &t);
        assert_eq!(plan.host_count(), 1);
        assert_eq!(plan.skipped.len(), 3);
        assert!(plan.folders[0].existing.is_some());
        editor.import(&plan, &|_, _| {}).unwrap();
        let t = tree(&editor);
        assert_eq!(t.find("db").unwrap().0.label(), "生产");
        assert_eq!(t.folders().filter(|f| f.label() == "生产").count(), 1);
    }

    #[test]
    fn host_keys_are_added_once() {
        let Some((home, editor)) = setup() else { return };
        let key = crate::known_hosts::HostKey {
            hosts: vec!["10.0.0.5".into(), "web01".into()],
            port: 22,
            key_type: "ssh-ed25519".into(),
            key: "AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl".into(),
        };
        std::fs::write(editor.known_hosts(), "# mine").unwrap();
        crate::acl::restrict_to_owner(&editor.known_hosts()).unwrap();
        assert_eq!(editor.add_host_keys(std::slice::from_ref(&key)).unwrap(), 1);
        assert_eq!(editor.add_host_keys(std::slice::from_ref(&key)).unwrap(), 0, "already there");
        let text = std::fs::read_to_string(editor.known_hosts()).unwrap();
        assert_eq!(text, format!("# mine\n{}\n", key.line()));
        let _ = home;
    }

    #[test]
    fn a_folder_that_fails_is_rolled_back() {
        let Some((home, editor)) = setup() else { return };
        let config = home.path().join("crt");
        let crt = config.join("Sessions");
        crt_file(&crt, "A/a1.ini", "S:\"Hostname\"=10.0.0.1\r\n");
        crt_file(&crt, "B/b1.ini", "S:\"Hostname\"=10.0.0.2\r\n");
        let scan = securecrt::scan(&config).unwrap();
        let plan = securecrt::plan(&scan, &tree(&editor));
        // meanwhile a file read earlier defines b1 differently
        let earlier = editor.folders_dir().join("0-mine.conf");
        std::fs::create_dir_all(editor.folders_dir()).unwrap();
        std::fs::write(&earlier, "Host b1
    HostName 10.9.9.9
").unwrap();
        crate::acl::restrict_to_owner(&earlier).unwrap();
        let outcome = editor.import(&plan, &|_, _| {}).unwrap();
        assert_eq!(outcome.written.len(), 1);
        assert_eq!(outcome.failed.len(), 1, "{:?}", outcome.failed);
        assert_eq!(outcome.failed[0].0, "B");
        let t = tree(&editor);
        assert!(t.find("a1").is_some());
        assert!(!t.folders().any(|f| f.label() == "B"), "the failed folder's file is gone");
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
