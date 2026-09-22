//! Who has the data directory open.
//!
//! A data directory can live on a share, a synced folder or a USB drive
//! and be opened from two machines at once (see "Data directory on shared
//! or synced storage" in `docs/ARCHITECTURE.md`). Writes are made atomic
//! so no file is ever half written, but two NativeTerms still overwrite
//! each other's settings, so the second one is told.
//!
//! The lock is the file's sharing mode (a lock on it elsewhere), not its
//! contents: `nativeterm.lock` is held open for writing while NativeTerm
//! runs, and Windows refuses a second writer — on a local disk and over
//! SMB alike. Its text is only there to name the holder; a stale file
//! from a machine that lost power blocks nothing, because nobody holds
//! it open any more.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

/// Others may read it (to see who we are), not write it.
#[cfg(windows)]
const FILE_SHARE_READ: u32 = 0x0000_0001;
/// The file, by name.
pub const LOCK_FILE: &str = "nativeterm.lock";

/// What a Windows editor puts in front of a UTF-8 file.
pub(crate) const BOM: char = '\u{feff}';

/// The data directory, held for as long as this lives.
pub struct DataLock {
    file: Option<File>,
    path: PathBuf,
}

impl DataLock {
    /// The file this lock is on (tests).
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for DataLock {
    fn drop(&mut self) {
        self.file = None; // closed before the file is removed
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Whoever had it first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Holder {
    pub machine: String,
    pub user: String,
    pub pid: String,
    pub since: String,
}

impl Holder {
    /// `machine` when it says one, else something that still names it.
    #[must_use]
    pub fn describe(&self) -> String {
        let who = match (self.machine.is_empty(), self.user.is_empty()) {
            (false, false) => format!("{} ({})", self.machine, self.user),
            (false, true) => self.machine.clone(),
            (true, false) => self.user.clone(),
            (true, true) => String::new(),
        };
        match (who.is_empty(), self.since.is_empty()) {
            (false, false) => format!("{who}, {}", self.since),
            (false, true) => who,
            (true, false) => self.since.clone(),
            (true, true) => t!("data-lock-unknown"),
        }
    }
}

use crate::t;

/// What `take` found.
pub enum Taken {
    /// Ours now.
    Ours(DataLock),
    /// Another NativeTerm has it (its own words).
    Busy(Holder),
    /// It could not be taken for another reason (a read-only folder, a
    /// filesystem without sharing modes): NativeTerm carries on.
    Unavailable(io::Error),
}

/// Take the data directory's lock, or find out who holds it.
pub fn take(dir: &Path) -> Taken {
    let path = dir.join(LOCK_FILE);
    match open_held(&path) {
        Ok(Some(mut file)) => {
            let _ = write!(file, "{}", ours());
            let _ = file.flush();
            Taken::Ours(DataLock { file: Some(file), path })
        }
        Ok(None) => Taken::Busy(read(&path)),
        Err(e) => Taken::Unavailable(e),
    }
}

/// The file, open and ours alone; `None` when someone else holds it.
#[cfg(windows)]
fn open_held(path: &Path) -> io::Result<Option<File>> {
    match OpenOptions::new().write(true).create(true).truncate(true).share_mode(FILE_SHARE_READ).open(path) {
        Ok(file) => Ok(Some(file)),
        // ERROR_SHARING_VIOLATION: someone holds it open
        Err(e) if e.raw_os_error() == Some(32) => Ok(None),
        Err(e) => Err(e),
    }
}

/// The file with an exclusive lock on it (advisory, so only NativeTerms
/// see it; a holder that died released it).
#[cfg(not(windows))]
fn open_held(path: &Path) -> io::Result<Option<File>> {
    let file = OpenOptions::new().write(true).create(true).truncate(false).open(path)?;
    match file.try_lock() {
        Ok(()) => {
            file.set_len(0)?;
            Ok(Some(file))
        }
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(e)) => Err(e),
    }
}

/// What this NativeTerm writes into the file.
fn ours() -> String {
    let machine = native_term_os::host::name();
    let user = native_term_os::host::user();
    let (date, time) = crate::utc_now();
    format!("machine = {machine:?}\nuser = {user:?}\npid = {}\nsince = \"{date} {time}\"\n", std::process::id())
}

/// The holder's own words (a file we can read but not write).
fn read(path: &Path) -> Holder {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    parse(&text)
}

fn parse(text: &str) -> Holder {
    let mut holder = Holder::default();
    // a file someone else wrote may start with a byte order mark
    for line in text.trim_start_matches(BOM).lines() {
        let Some((key, value)) = line.split_once('=') else { continue };
        let value = value.trim().trim_matches('"').to_string();
        match key.trim() {
            "machine" => holder.machine = value,
            "user" => holder.user = value,
            "pid" => holder.pid = value,
            "since" => holder.since = value,
            _ => {}
        }
    }
    holder
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_second_one_is_told_who_has_it() {
        let dir = tempfile::tempdir().unwrap();
        let first = match take(dir.path()) {
            Taken::Ours(lock) => lock,
            _ => panic!("an empty folder is free"),
        };
        let Taken::Busy(holder) = take(dir.path()) else { panic!("taken twice") };
        assert_eq!(holder.machine, native_term_os::host::name());
        assert_eq!(holder.pid, std::process::id().to_string());
        assert!(!holder.describe().is_empty());
        // and it is free again afterwards
        drop(first);
        assert!(!dir.path().join(LOCK_FILE).exists(), "the file goes with the lock");
        assert!(matches!(take(dir.path()), Taken::Ours(_)));
    }

    #[test]
    fn a_file_left_behind_blocks_nothing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(LOCK_FILE), "machine = \"GONE\"\npid = 1\n").unwrap();
        assert!(matches!(take(dir.path()), Taken::Ours(_)), "nobody holds it open");
    }

    #[test]
    fn a_file_written_by_an_editor_still_reads() {
        let text = format!("{BOM}machine = \"WORK-PC\"\r\nuser = \"someone\"\r\n");
        assert_eq!(parse(&text).machine, "WORK-PC");
    }

    #[test]
    fn a_holder_is_named_by_whatever_it_said() {
        let holder = parse("machine = \"WORK-PC\"\nuser = \"simon\"\npid = 42\nsince = \"2026-09-21 10:00:00\"\n");
        assert_eq!(holder.pid, "42");
        assert_eq!(holder.describe(), "WORK-PC (simon), 2026-09-21 10:00:00");
        assert_eq!(parse("").describe(), t!("data-lock-unknown"));
        assert_eq!(parse("machine = \"WORK-PC\"").describe(), "WORK-PC");
    }
}
