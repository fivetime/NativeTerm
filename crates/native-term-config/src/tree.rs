//! The session tree: `~/.ssh/config` plus every file it includes.
//! Each included file is one folder; each `Host` block with at least one
//! literal name is one session. Effective settings are ssh's business
//! (`ssh -G`); this tree only shows and edits what is written.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use crate::alias::is_literal;
use crate::document::{BlockKind, Document};
use crate::include::{self, IncludeError};

/// Reserved pseudo-host carrying a folder's default `NativeTerm*` keys.
pub const FOLDER_DEFAULTS_HOST: &str = "__nativeterm_folder__";
const KEY_PREFIX: &str = "nativeterm";
/// ssh's own nesting limit for `Include`.
const MAX_INCLUDE_DEPTH: usize = 16;

/// `NativeTerm*` keys, stored lowercase without the prefix
/// (`NativeTermTabColor` → `tabcolor`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NtKeys(pub BTreeMap<String, String>);

impl NtKeys {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(&key.to_ascii_lowercase()).map(String::as_str)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostEntry {
    /// Literal names from the `Host` line; the first is the one NativeTerm
    /// connects with.
    pub aliases: Vec<String>,
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub proxy_jump: Option<String>,
    pub identity_files: Vec<String>,
    pub nt: NtKeys,
    /// File and line of the `Host` header.
    pub file: PathBuf,
    pub line: usize,
}

impl HostEntry {
    pub fn alias(&self) -> &str {
        &self.aliases[0]
    }

    pub fn label(&self) -> &str {
        self.nt.get("label").unwrap_or(self.alias())
    }

    pub fn id(&self) -> Option<&str> {
        self.nt.get("id")
    }

    /// Where ssh will connect: `HostName`, or the alias itself.
    pub fn target(&self) -> &str {
        self.hostname.as_deref().unwrap_or(self.alias())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Folder {
    /// File stem (`ceph-cluster` for `ceph-cluster.conf`); empty for the
    /// main config's own hosts.
    pub name: String,
    pub file: PathBuf,
    /// Keys from `Host __nativeterm_folder__`.
    pub defaults: NtKeys,
    pub hosts: Vec<HostEntry>,
}

impl Folder {
    pub fn label(&self) -> &str {
        self.defaults.get("label").unwrap_or(&self.name)
    }

    /// A host's `NativeTerm*` key, falling back to the folder default.
    pub fn nt<'a>(&'a self, host: &'a HostEntry, key: &str) -> Option<&'a str> {
        host.nt.get(key).or_else(|| self.defaults.get(key))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Warning {
    IncludeInsideBlock { file: PathBuf, line: usize },
    WildcardIncludeDirectory { file: PathBuf, pattern: String },
    IncludeTooDeep { file: PathBuf },
    InvalidPort { file: PathBuf, line: usize, value: String },
    DuplicateAlias { alias: String, first: PathBuf, again: PathBuf },
    Unreadable { file: PathBuf, error: String },
}

/// A `Host` block that isn't a session but settings shared by several
/// hosts, e.g. `Host node01 node02 incus-node-*` with only `User root`.
/// Its effect shows in `ssh -G`; the UI lists it separately.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharedSettings {
    pub patterns: Vec<String>,
    pub file: PathBuf,
    pub line: usize,
}

#[derive(Clone, Debug, Default)]
pub struct SessionTree {
    /// Hosts written directly in `~/.ssh/config`.
    pub root: Option<Folder>,
    /// One per included file, in include order.
    pub folders: Vec<Folder>,
    pub shared: Vec<SharedSettings>,
    pub warnings: Vec<Warning>,
}

impl SessionTree {
    /// Load from `ssh_dir` (normally `~/.ssh`); `~` is its parent.
    pub fn load(ssh_dir: &Path) -> SessionTree {
        let home = ssh_dir.parent().unwrap_or(ssh_dir).to_path_buf();
        Self::load_with(ssh_dir, &home)
    }

    pub fn load_with(ssh_dir: &Path, home: &Path) -> SessionTree {
        let mut loader = Loader {
            ssh_dir,
            home,
            tree: SessionTree::default(),
            visited: HashSet::new(),
            candidates: Vec::new(),
        };
        loader.visit(&ssh_dir.join("config"), 0, true);
        loader.classify();
        loader.check_duplicates();
        loader.tree
    }

    pub fn folders(&self) -> impl Iterator<Item = &Folder> {
        self.root.iter().chain(self.folders.iter())
    }

    pub fn hosts(&self) -> impl Iterator<Item = (&Folder, &HostEntry)> {
        self.folders().flat_map(|f| f.hosts.iter().map(move |h| (f, h)))
    }

    /// Every literal alias in lowercase, for generating new unique ones.
    pub fn taken_aliases(&self) -> HashSet<String> {
        self.hosts().flat_map(|(_, h)| h.aliases.iter().map(|a| a.to_ascii_lowercase())).collect()
    }

    pub fn find(&self, alias: &str) -> Option<(&Folder, &HostEntry)> {
        self.hosts().find(|(_, h)| h.aliases.iter().any(|a| a.eq_ignore_ascii_case(alias)))
    }
}

/// A `Host` block with at least one literal name, before deciding whether
/// it is a session or shared settings.
struct Candidate {
    /// `None` for the main config, else index into `tree.folders`.
    folder: Option<usize>,
    entry: HostEntry,
    patterns: Vec<String>,
    has_wildcard: bool,
}

struct Loader<'a> {
    ssh_dir: &'a Path,
    home: &'a Path,
    tree: SessionTree,
    visited: HashSet<PathBuf>,
    candidates: Vec<Candidate>,
}

impl Loader<'_> {
    fn visit(&mut self, path: &Path, depth: usize, is_main: bool) {
        if depth > MAX_INCLUDE_DEPTH {
            self.tree.warnings.push(Warning::IncludeTooDeep { file: path.to_path_buf() });
            return;
        }
        if !self.visited.insert(path.to_path_buf()) {
            return;
        }
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if is_main && e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => {
                self.tree.warnings.push(Warning::Unreadable { file: path.to_path_buf(), error: e.to_string() });
                return;
            }
        };
        let doc = Document::parse(&text);
        let name = if is_main {
            String::new()
        } else {
            let file_name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            file_name.strip_suffix(".conf").map(str::to_string).unwrap_or(file_name)
        };
        let mut folder = Folder { name, file: path.to_path_buf(), defaults: NtKeys::default(), hosts: Vec::new() };
        let folder_index = if is_main { None } else { Some(self.tree.folders.len()) };
        let mut includes = Vec::new();

        for block in doc.blocks() {
            for (line, d) in doc.directives(&block) {
                if d.is("Include") {
                    if block.kind == BlockKind::Global {
                        includes.extend(d.args.iter().cloned());
                    } else {
                        self.tree.warnings.push(Warning::IncludeInsideBlock { file: path.to_path_buf(), line });
                    }
                }
            }
            let BlockKind::Host(patterns) = &block.kind else { continue };
            let nt = nt_keys(&doc, &block);
            if patterns.len() == 1 && patterns[0].eq_ignore_ascii_case(FOLDER_DEFAULTS_HOST) {
                folder.defaults = nt;
                continue;
            }
            let aliases: Vec<String> = patterns.iter().filter(|p| is_literal(p)).cloned().collect();
            if aliases.is_empty() {
                continue;
            }
            let first = |kw: &str| doc.directives(&block).map(|(_, d)| d).find(|d| d.is(kw)).map(|d| d.value());
            let header = block.header.unwrap_or(0);
            let port = first("Port").and_then(|p| match p.parse::<u16>() {
                Ok(n) => Some(n),
                Err(_) => {
                    self.tree.warnings.push(Warning::InvalidPort { file: path.to_path_buf(), line: header, value: p });
                    None
                }
            });
            let entry = HostEntry {
                aliases,
                hostname: first("HostName"),
                user: first("User"),
                port,
                proxy_jump: first("ProxyJump"),
                identity_files: doc.directives(&block).filter(|(_, d)| d.is("IdentityFile")).map(|(_, d)| d.value()).collect(),
                nt,
                file: path.to_path_buf(),
                line: header,
            };
            self.candidates.push(Candidate {
                folder: folder_index,
                has_wildcard: patterns.len() != entry.aliases.len(),
                patterns: patterns.clone(),
                entry,
            });
        }

        if is_main {
            self.tree.root = Some(folder);
        } else {
            self.tree.folders.push(folder);
        }
        for pattern in includes {
            match include::resolve(&pattern, self.home, self.ssh_dir) {
                Ok(files) => files.iter().for_each(|f| self.visit(f, depth + 1, false)),
                Err(IncludeError::WildcardDirectory(pattern)) => {
                    self.tree.warnings.push(Warning::WildcardIncludeDirectory { file: path.to_path_buf(), pattern })
                }
            }
        }
    }

    /// A block is a session if it sets `HostName`, or if it names only
    /// hosts that no `HostName` block defines (ssh then connects to the
    /// name itself). Everything else is shared settings: blocks with
    /// wildcards, or ones listing hosts defined elsewhere. Done after all
    /// files are read, since the defining block may be in another file.
    fn classify(&mut self) {
        let defined: HashSet<String> = self
            .candidates
            .iter()
            .filter(|c| c.entry.hostname.is_some())
            .flat_map(|c| c.entry.aliases.iter().map(|a| a.to_ascii_lowercase()))
            .collect();
        for c in std::mem::take(&mut self.candidates) {
            let references_defined = c.entry.aliases.iter().any(|a| defined.contains(&a.to_ascii_lowercase()));
            let is_session = c.entry.hostname.is_some() || (!c.has_wildcard && !references_defined);
            if !is_session {
                self.tree.shared.push(SharedSettings { patterns: c.patterns, file: c.entry.file, line: c.entry.line });
                continue;
            }
            let folder = match c.folder {
                Some(i) => &mut self.tree.folders[i],
                None => self.tree.root.as_mut().expect("main config visited first"),
            };
            folder.hosts.push(c.entry);
        }
    }

    fn check_duplicates(&mut self) {
        let mut seen: BTreeMap<String, PathBuf> = BTreeMap::new();
        let mut warnings = Vec::new();
        for (_, host) in self.tree.hosts() {
            for alias in &host.aliases {
                match seen.get(&alias.to_ascii_lowercase()) {
                    Some(first) => warnings.push(Warning::DuplicateAlias {
                        alias: alias.clone(),
                        first: first.clone(),
                        again: host.file.clone(),
                    }),
                    None => {
                        seen.insert(alias.to_ascii_lowercase(), host.file.clone());
                    }
                }
            }
        }
        self.tree.warnings.extend(warnings);
    }
}

fn nt_keys(doc: &Document, block: &crate::document::Block) -> NtKeys {
    let mut keys = BTreeMap::new();
    for (_, d) in doc.directives(block) {
        let lower = d.keyword.to_ascii_lowercase();
        if let Some(key) = lower.strip_prefix(KEY_PREFIX) {
            if !key.is_empty() {
                // first value wins, as in ssh
                keys.entry(key.to_string()).or_insert_with(|| d.value());
            }
        }
    }
    NtKeys(keys)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn fixture() -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        let ssh = home.path().join(".ssh");
        write(
            &ssh.join("config"),
            "IgnoreUnknown NativeTerm*\n\
             Include ~/.ssh/config.d/*.conf\n\
             \n\
             Host node01 incus-node-01\n    HostName 10.32.32.130\n    Port 2222\n\n\
             Host node01 node02 incus-node-*\n    User root\n\n\
             Host *\n    ServerAliveInterval 30\n    Include never-a-folder.conf\n",
        );
        write(
            &ssh.join("config.d/ceph-cluster.conf"),
            "Host __nativeterm_folder__\n    NativeTermLabel \"Ceph 集群\"\n    NativeTermTabColor #C0392B\n\n\
             Host ceph-cluster.osp-control1\n    HostName 10.32.16.66\n    User ops\n    \
             NativeTermLabel \"10.32.16.66(osp-control1)\"\n    NativeTermId 7f1c\n    NativeTermTabColor #2E86C1\n    \
             IdentityFile ~/.ssh/a\n    IdentityFile ~/.ssh/b\n\n\
             Host ceph-cluster.osd1\n    HostName 10.32.16.70\n    Port nope\n",
        );
        write(
            &ssh.join("config.d/k8s.conf"),
            "Host k8s-master\n    HostName 10.32.32.66\n\n\
             Host node01\n    User admin\n\n\
             Host k8s-worker1\n\n\
             Host ceph-cluster.osd1\n    HostName 10.99.0.1\n",
        );
        write(&ssh.join("config.d/switches.nt.toml"), "not an ssh file");
        home
    }

    #[test]
    fn builds_folders_and_hosts() {
        let home = fixture();
        let tree = SessionTree::load(&home.path().join(".ssh"));

        let root = tree.root.as_ref().unwrap();
        assert_eq!(root.hosts.len(), 1, "`Host *` and the shared block are not sessions");
        assert_eq!(root.hosts[0].aliases, vec!["node01", "incus-node-01"]);
        assert_eq!(root.hosts[0].port, Some(2222));

        // shared settings: wildcard block (main config) and a block naming a
        // host defined elsewhere (k8s.conf)
        let shared: Vec<Vec<String>> = tree.shared.iter().map(|s| s.patterns.clone()).collect();
        assert_eq!(shared, vec![vec!["node01".to_string(), "node02".into(), "incus-node-*".into()], vec!["node01".into()]]);

        let k8s: Vec<&str> = tree.folders[1].hosts.iter().map(|h| h.alias()).collect();
        assert_eq!(k8s, vec!["k8s-master", "k8s-worker1", "ceph-cluster.osd1"], "no HostName and not defined elsewhere: a session");

        let names: Vec<&str> = tree.folders.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["ceph-cluster", "k8s"], "only *.conf, sorted");

        let ceph = &tree.folders[0];
        assert_eq!(ceph.label(), "Ceph 集群");
        assert_eq!(ceph.hosts.len(), 2, "folder defaults are not a host");
        let control = &ceph.hosts[0];
        assert_eq!(control.label(), "10.32.16.66(osp-control1)");
        assert_eq!(control.id(), Some("7f1c"));
        assert_eq!(control.identity_files, vec!["~/.ssh/a", "~/.ssh/b"]);
        assert_eq!(ceph.nt(control, "TabColor"), Some("#2E86C1"), "own key wins");
        assert_eq!(ceph.nt(&ceph.hosts[1], "tabcolor"), Some("#C0392B"), "folder default");
        assert_eq!(ceph.hosts[1].label(), "ceph-cluster.osd1");
        assert_eq!(ceph.hosts[1].port, None);

        let (folder, host) = tree.find("CEPH-CLUSTER.OSD1").unwrap();
        assert_eq!(folder.name, "ceph-cluster", "first definition wins, as in ssh");
        assert_eq!(host.target(), "10.32.16.70");
        assert!(tree.taken_aliases().contains("k8s-master"));
    }

    #[test]
    fn reports_problems() {
        let home = fixture();
        let tree = SessionTree::load(&home.path().join(".ssh"));
        assert!(tree.warnings.iter().any(|w| matches!(w, Warning::IncludeInsideBlock { .. })));
        assert!(tree.warnings.iter().any(|w| matches!(w, Warning::InvalidPort { value, .. } if value == "nope")));
        let dups: Vec<&str> = tree
            .warnings
            .iter()
            .filter_map(|w| match w {
                Warning::DuplicateAlias { alias, .. } => Some(alias.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(dups, vec!["ceph-cluster.osd1"], "only two session blocks with one name clash");
    }

    #[test]
    fn missing_config_is_an_empty_tree() {
        let home = tempfile::tempdir().unwrap();
        let tree = SessionTree::load(&home.path().join(".ssh"));
        assert!(tree.root.as_ref().unwrap().hosts.is_empty());
        assert!(tree.folders.is_empty());
        assert!(tree.warnings.is_empty());
    }

    #[test]
    fn include_cycles_are_visited_once() {
        let home = tempfile::tempdir().unwrap();
        let ssh = home.path().join(".ssh");
        write(&ssh.join("config"), "Include a.conf\n");
        write(&ssh.join("a.conf"), "Include b.conf\nHost a\n");
        write(&ssh.join("b.conf"), "Include a.conf\nHost b\n");
        let tree = SessionTree::load(&ssh);
        let names: Vec<&str> = tree.folders.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b"]);
    }
}
