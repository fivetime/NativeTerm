//! Non-SSH sessions (Telnet, raw, serial) run through ntplink, a Windows
//! program: here there is none, so no alias is one of those. The folder
//! settings the rest of the shim asks this module for are the same.

// a stand-in carries the whole API, used or not
#![allow(dead_code)]

use std::path::PathBuf;

use native_term_config::plink::PlinkSession;

use crate::link::Link;

/// The session `alias`, if it is a non-SSH one: never, here.
pub fn lookup(_alias: &str) -> Option<PlinkSession> {
    None
}

static SSH_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// `--ssh-dir`: NativeTerm runs on another folder than `~/.ssh`.
pub fn set_ssh_dir(dir: PathBuf) {
    let _ = SSH_DIR.set(dir);
}

/// `--ssh-dir` or `NATIVETERM_SSH_DIR` (tests): a folder other than
/// `~/.ssh`.
pub fn custom_ssh_dir() -> Option<PathBuf> {
    if let Some(dir) = SSH_DIR.get() {
        return Some(dir.clone());
    }
    std::env::var_os("NATIVETERM_SSH_DIR").filter(|d| !d.is_empty()).map(PathBuf::from)
}

/// `--ssh-dir`, `NATIVETERM_SSH_DIR` (tests), or `~/.ssh`.
pub fn ssh_dir() -> PathBuf {
    custom_ssh_dir().or_else(native_term_os::home::ssh_dir).unwrap_or_default()
}

/// Never reached: `lookup` finds nothing.
pub fn run(_alias: &str, _link: Option<&Link>, _flags: crate::args::Flags) -> i32 {
    1
}
