//! Reads and writes `~/.ssh/config` and its `Include`-d `config.d/*.conf`
//! files. This is the single source of truth for saved sessions — see
//! `docs/ARCHITECTURE.md` for why there is no separate database.
//!
//! Planned surface (not yet implemented):
//! - `SessionTree::load(ssh_dir: &Path) -> SessionTree` — parse the main
//!   config plus every `Include`-d file into a folder/host tree.
//! - `SessionTree::save_host(&mut self, folder: &str, host: HostEntry)` —
//!   format-preserving write-back (targeted edits, not a full
//!   reserialize) so hand-maintained comments/spacing survive.

pub struct HostEntry {
    pub alias: String,
    pub hostname: String,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<String>,
}

pub struct SessionTree {
    pub folders: Vec<Folder>,
}

pub struct Folder {
    pub name: String,
    pub hosts: Vec<HostEntry>,
}
