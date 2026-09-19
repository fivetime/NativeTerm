//! Comparing a local folder with a server's folder, everything below
//! them, and what making them alike takes.
//!
//! Files are matched by name (a server's name as it would be written on
//! Windows, see [`local_name`]) and compared by size and modification
//! time: transfers keep modification times, so a copied file compares
//! the same afterwards. A folder on one side only is one entry (copied or
//! deleted whole); folders on both sides are gone into.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::transfer::{local_name, PART};
use crate::wire::Attrs;
use crate::{join, Error, Names, Result, Session};

/// Modification times this close are the same (FAT keeps 2 seconds).
pub const SLACK: u64 = 2;

/// A file or folder as one side has it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Meta {
    pub dir: bool,
    pub size: u64,
    pub modified: Option<u64>,
}

/// A file or folder on either side, or both.
#[derive(Clone, Debug)]
pub struct Found {
    /// Below the compared folders, `/`-separated, for showing.
    pub rel: String,
    pub local: PathBuf,
    /// Empty when a local name can't be written in the server's encoding
    /// (such an entry is a [`Act::Conflict`]).
    pub remote: Vec<u8>,
    pub here: Option<Meta>,
    pub there: Option<Meta>,
    /// The server's attributes (for downloading it).
    pub attrs: Option<Attrs>,
}

/// Which side is copied to which.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The local folder is the source.
    Upload,
    /// The server's folder is the source.
    Download,
    /// Both ways: what one side lacks is copied to it, and of two
    /// different files the newer one wins.
    Both,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    Upload,
    Download,
    DeleteLocal,
    DeleteRemote,
    /// Nothing to do: the same on both sides.
    Same,
    /// Nothing done: only on the target side, and extra files are kept.
    Skip,
    /// Nothing done: a file on one side and a folder on the other, a
    /// name the server can't take, or two different files with the same
    /// time (which is newer?).
    Conflict,
}

impl Found {
    /// Whether the two files are the same (size and time).
    pub fn same(&self) -> bool {
        match (&self.here, &self.there) {
            (Some(a), Some(b)) if !a.dir && !b.dir => {
                a.size == b.size
                    && match (a.modified, b.modified) {
                        (Some(x), Some(y)) => x.abs_diff(y) <= SLACK,
                        _ => true,
                    }
            }
            _ => false,
        }
    }

    /// What `mode` does with it; `delete_extra`: what the target has and
    /// the source doesn't is deleted (one-way modes only).
    pub fn act(&self, mode: Mode, delete_extra: bool) -> Act {
        if self.remote.is_empty() {
            return Act::Conflict;
        }
        match (&self.here, &self.there) {
            (Some(a), Some(b)) if a.dir != b.dir => Act::Conflict,
            (Some(_), Some(_)) if self.same() => Act::Same,
            (Some(a), Some(b)) => match mode {
                Mode::Upload => Act::Upload,
                Mode::Download => Act::Download,
                Mode::Both => match (a.modified, b.modified) {
                    (Some(x), Some(y)) if x > y + SLACK => Act::Upload,
                    (Some(x), Some(y)) if y > x + SLACK => Act::Download,
                    _ => Act::Conflict,
                },
            },
            (Some(_), None) => match mode {
                Mode::Upload | Mode::Both => Act::Upload,
                Mode::Download if delete_extra => Act::DeleteLocal,
                Mode::Download => Act::Skip,
            },
            (None, Some(_)) => match mode {
                Mode::Download | Mode::Both => Act::Download,
                Mode::Upload if delete_extra => Act::DeleteRemote,
                Mode::Upload => Act::Skip,
            },
            (None, None) => Act::Same,
        }
    }
}

/// Compares `local` with the server's `remote`, everything below them.
/// `seen` counts what was looked at (for showing); `stop` ends it.
pub fn compare(
    sftp: &Session,
    names: &Names,
    local: &Path,
    remote: &[u8],
    seen: &AtomicUsize,
    stop: &AtomicBool,
) -> Result<Vec<Found>> {
    let mut out = Vec::new();
    walk(sftp, names, local, remote, "", seen, stop, &mut out)?;
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn walk(
    sftp: &Session,
    names: &Names,
    local: &Path,
    remote: &[u8],
    rel: &str,
    seen: &AtomicUsize,
    stop: &AtomicBool,
    out: &mut Vec<Found>,
) -> Result<()> {
    if stop.load(Ordering::Relaxed) {
        return Err(Error::Cancelled);
    }
    // name as on Windows → (server's name, its attributes)
    let mut theirs: Vec<(String, Vec<u8>, Attrs)> = Vec::new();
    for entry in sftp.read_dir(remote)? {
        let path = join(remote, &entry.name);
        // a link: what it points to; a link to a folder isn't gone into
        // (no loops), a dangling one is left out
        let attrs = if entry.attrs.is_symlink() {
            match sftp.stat(&path) {
                Ok(a) if a.is_dir() => continue,
                Ok(a) => a,
                Err(_) => continue,
            }
        } else {
            entry.attrs
        };
        if !entry.name.ends_with(PART.as_bytes()) {
            theirs.push((local_name(names, &entry.name), entry.name, attrs));
        }
    }
    let mut ours: Vec<(String, std::fs::Metadata)> = Vec::new();
    for entry in std::fs::read_dir(local)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        // what `metadata` follows (a link to a file is the file)
        if let (false, Ok(meta)) = (name.ends_with(PART), std::fs::metadata(entry.path())) {
            ours.push((name, meta));
        }
    }
    seen.fetch_add(theirs.len() + ours.len(), Ordering::Relaxed);
    let mut keys: Vec<String> =
        ours.iter().map(|(n, _)| n.clone()).chain(theirs.iter().map(|(n, ..)| n.clone())).collect();
    keys.sort();
    keys.dedup();
    for key in keys {
        let here = ours.iter().find(|(n, _)| *n == key).map(|(_, m)| Meta {
            dir: m.is_dir(),
            size: if m.is_dir() { 0 } else { m.len() },
            modified: m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()),
        });
        let theirs = theirs.iter().find(|(n, ..)| *n == key);
        let there = theirs.map(|(_, _, a)| Meta {
            dir: a.is_dir(),
            size: if a.is_dir() { 0 } else { a.size.unwrap_or(0) },
            modified: a.atime_mtime.map(|(_, m)| m as u64),
        });
        let remote_path = match theirs {
            Some((_, name, _)) => join(remote, name),
            None => names.encode(&key).map(|n| join(remote, &n)).unwrap_or_default(),
        };
        let local_path = local.join(&key);
        let rel = if rel.is_empty() { key.clone() } else { format!("{rel}/{key}") };
        if here.as_ref().is_some_and(|m| m.dir) && there.as_ref().is_some_and(|m| m.dir) {
            walk(sftp, names, &local_path, &remote_path, &rel, seen, stop, out)?;
            continue;
        }
        out.push(Found {
            rel,
            local: local_path,
            remote: remote_path,
            here,
            there,
            attrs: theirs.map(|t| t.2.clone()),
        });
    }
    Ok(())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::process::Command;
    use std::time::{Duration, UNIX_EPOCH};

    fn found(here: Option<(bool, u64, u64)>, there: Option<(bool, u64, u64)>) -> Found {
        let meta = |(dir, size, t): (bool, u64, u64)| Meta { dir, size, modified: Some(t) };
        Found {
            rel: "f".into(),
            local: PathBuf::from("f"),
            remote: b"/f".to_vec(),
            here: here.map(meta),
            there: there.map(meta),
            attrs: None,
        }
    }

    #[test]
    fn what_each_mode_does() {
        let same = found(Some((false, 5, 100)), Some((false, 5, 101)));
        assert!(same.same());
        assert_eq!(same.act(Mode::Both, true), Act::Same);
        let newer_here = found(Some((false, 5, 200)), Some((false, 5, 100)));
        assert_eq!(newer_here.act(Mode::Both, false), Act::Upload);
        assert_eq!(newer_here.act(Mode::Download, false), Act::Download);
        let newer_there = found(Some((false, 5, 100)), Some((false, 9, 200)));
        assert_eq!(newer_there.act(Mode::Both, false), Act::Download);
        assert_eq!(newer_there.act(Mode::Upload, false), Act::Upload);
        // same time, different sizes: which is newer can't be told
        assert_eq!(found(Some((false, 5, 100)), Some((false, 6, 100))).act(Mode::Both, false), Act::Conflict);
        let only_here = found(Some((false, 5, 100)), None);
        assert_eq!(only_here.act(Mode::Both, true), Act::Upload);
        assert_eq!(only_here.act(Mode::Download, false), Act::Skip);
        assert_eq!(only_here.act(Mode::Download, true), Act::DeleteLocal);
        let only_there = found(None, Some((true, 0, 100)));
        assert_eq!(only_there.act(Mode::Upload, true), Act::DeleteRemote);
        assert_eq!(only_there.act(Mode::Both, true), Act::Download);
        assert_eq!(found(Some((true, 0, 1)), Some((false, 3, 1))).act(Mode::Upload, false), Act::Conflict);
        let mut unnamed = only_here.clone();
        unnamed.remote.clear();
        assert_eq!(unnamed.act(Mode::Upload, false), Act::Conflict);
    }

    fn set_time(path: &Path, secs: u64) {
        let f = std::fs::File::options().write(true).open(path).unwrap();
        f.set_modified(UNIX_EPOCH + Duration::from_secs(secs)).unwrap();
    }

    /// Against Windows' own sftp-server: a tree with a file the same on
    /// both sides, one changed, ones on one side only, a folder on one
    /// side only (one entry), a partial copy (left out) and CJK names.
    #[test]
    fn two_trees() {
        let server = Path::new(r"C:\Windows\System32\OpenSSH\sftp-server.exe");
        if !server.exists() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut command = Command::new(server);
        command.arg("-d").arg(dir.path());
        let sftp = Session::spawn(command).unwrap();
        let (a, b) = (dir.path().join("本地"), dir.path().join("服务器"));
        for root in [&a, &b] {
            std::fs::create_dir_all(root.join("共同").join("深")).unwrap();
            std::fs::write(root.join("same.txt"), b"same").unwrap();
            set_time(&root.join("same.txt"), 1_600_000_000);
            std::fs::write(root.join("共同").join("深").join("改.txt"), b"v1").unwrap();
        }
        set_time(&a.join("共同").join("深").join("改.txt"), 1_700_000_000);
        std::fs::write(b.join("共同").join("深").join("改.txt"), b"v2 longer").unwrap();
        set_time(&b.join("共同").join("深").join("改.txt"), 1_600_000_000);
        std::fs::write(a.join("只在本地.txt"), b"x").unwrap();
        std::fs::create_dir_all(b.join("只在服务器").join("子")).unwrap();
        std::fs::write(b.join("只在服务器").join("子").join("f"), b"y").unwrap();
        std::fs::write(b.join(format!("half.bin{PART}")), b"partial").unwrap();

        let remote = format!("/{}", b.display().to_string().replace('\\', "/")).into_bytes();
        let (seen, stop) = (AtomicUsize::new(0), AtomicBool::new(false));
        let found = compare(&sftp, &Names::default(), &a, &remote, &seen, &stop).unwrap();
        let rels: Vec<&str> = found.iter().map(|f| f.rel.as_str()).collect();
        assert_eq!(rels, ["same.txt", "共同/深/改.txt", "只在服务器", "只在本地.txt"]);
        let acts: Vec<Act> = found.iter().map(|f| f.act(Mode::Both, false)).collect();
        assert_eq!(acts, [Act::Same, Act::Upload, Act::Download, Act::Upload]);
        let only_there = &found[2];
        assert!(only_there.there.as_ref().unwrap().dir && only_there.here.is_none());
        assert!(only_there.attrs.as_ref().unwrap().is_dir());
        assert!(found[3].remote.ends_with("只在本地.txt".as_bytes()));
        assert!(seen.load(Ordering::Relaxed) >= 8);

        stop.store(true, Ordering::Relaxed);
        assert!(matches!(compare(&sftp, &Names::default(), &a, &remote, &seen, &stop), Err(Error::Cancelled)));
    }
}
