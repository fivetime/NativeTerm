//! Reads and edits `~/.ssh/config` and its `Include`-d `config.d/*.conf`
//! files, the single source of truth for saved sessions (see
//! `docs/ARCHITECTURE.md`, "Session config storage").
//!
//! - [`document`]: line-preserving model of one file and format-preserving
//!   edits.
//! - [`tree`]: the folder/host tree across all files.
//! - [`header`]: the `IgnoreUnknown` / `Include` lines in the main config.
//! - [`alias`]: literal-pattern checks and unique alias generation.
//! - [`effective`]: effective settings through `ssh -G`.
//!
//! Safe writing (backups, ACLs, change detection, validation with rollback)
//! is a separate step.

pub mod alias;
pub mod document;
pub mod effective;
pub mod header;
pub mod include;
pub mod tree;

pub use document::Document;
pub use tree::{Folder, HostEntry, NtKeys, SessionTree, SharedSettings, Warning};

/// A new stable host id (`NativeTermId`).
pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}
