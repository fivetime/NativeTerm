//! Transfers of files and folders: first a plan (every file and folder,
//! and the total size, so progress is a fraction), then the copying, with
//! progress, pausing and cancelling. Also deleting a folder with what's in
//! it.
//!
//! A file is copied under its name plus [`PART`] and renamed when it is
//! complete, so a partial file never looks like the real one. A partial
//! file left by a pause, a lost connection or a closed window is
//! continued from where it ends by the next transfer of that file, unless
//! the source changed since it was written.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;

use crate::wire::Attrs;
use crate::{join, Error, Names, Result, Session};

/// One thing to create or copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub remote: Vec<u8>,
    pub local: PathBuf,
    /// A folder to create (before what's in it).
    pub dir: bool,
    pub size: u64,
    /// The remote file's permissions (downloads keep none; uploads of a
    /// new file get 0644, folders 0755).
    pub permissions: Option<u32>,
    /// The source's modification time (seconds since the Unix epoch): a
    /// partial copy older than it isn't continued.
    pub modified: Option<u64>,
}

/// What a file being copied is called until it is complete.
pub const PART: &str = ".ntpart";

/// A transfer's state, shared with whoever shows it.
#[derive(Default)]
pub struct Progress {
    pub done: AtomicU64,
    pub total: AtomicU64,
    pub cancel: AtomicBool,
    /// Stops like `cancel`, but the partial file stays to be continued.
    pub pause: AtomicBool,
    /// The first planned item not yet copied (where a paused or failed
    /// transfer goes on).
    pub next: AtomicUsize,
    /// Of `done`, what was copied before (by an earlier run, or found in
    /// a partial file): not counted for a speed.
    pub skipped: AtomicU64,
    /// The file being copied (for showing).
    pub current: Mutex<String>,
}

impl Progress {
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    pub fn paused(&self) -> bool {
        self.pause.load(Ordering::Relaxed)
    }

    /// Cancelled or paused.
    pub fn stopped(&self) -> bool {
        self.cancelled() || self.paused()
    }

    /// Before going on with a plan: `done` as the items before `next`
    /// add up to.
    fn restart(&self, items: &[Item]) -> usize {
        let next = self.next.load(Ordering::Relaxed).min(items.len());
        let before = items[..next].iter().map(|i| i.size).sum();
        self.done.store(before, Ordering::Relaxed);
        self.skipped.store(before, Ordering::Relaxed);
        next
    }

    fn set_current(&self, text: String) {
        *self.current.lock().unwrap_or_else(|e| e.into_inner()) = text;
    }
}

/// A name from the server as a Windows file name: decoded, and what
/// Windows doesn't allow in a name replaced (`< > : " / \ | ? *`, control
/// characters, trailing dots and spaces, device names like `CON`).
pub fn local_name(names: &Names, name: &[u8]) -> String {
    let text = names.decode(name);
    let mut out: String = text
        .chars()
        .map(|c| if c < ' ' || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') { '_' } else { c })
        .collect();
    while out.ends_with(['.', ' ']) {
        out.pop();
    }
    let stem = out.split('.').next().unwrap_or("").to_ascii_uppercase();
    let device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit());
    if device {
        out.insert(0, '_');
    }
    if out.is_empty() {
        out.push('_');
    }
    out
}

/// A local file's name for the server, in the host's encoding.
pub fn remote_name(names: &Names, path: &Path) -> Result<Vec<u8>> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    names
        .encode(&name)
        .ok_or_else(|| Error::Protocol(format!("\"{name}\" can't be written in the server's file name encoding")))
}

/// What downloading `remote` (a file or, with `attrs.is_dir()`, a folder
/// and everything below) into the folder `into` involves.
pub fn plan_download(
    sftp: &Session,
    names: &Names,
    remote: &[u8],
    attrs: &Attrs,
    into: &Path,
    progress: &Progress,
) -> Result<Vec<Item>> {
    let name = remote.rsplit(|&c| c == b'/').next().unwrap_or(remote);
    let local = into.join(local_name(names, name));
    let mut items = Vec::new();
    walk_remote(sftp, names, remote, attrs, local, &mut items, progress)?;
    Ok(items)
}

fn walk_remote(
    sftp: &Session,
    names: &Names,
    remote: &[u8],
    attrs: &Attrs,
    local: PathBuf,
    items: &mut Vec<Item>,
    progress: &Progress,
) -> Result<()> {
    if progress.stopped() {
        return Err(Error::Cancelled);
    }
    if !attrs.is_dir() {
        let size = attrs.size.unwrap_or(0);
        progress.total.fetch_add(size, Ordering::Relaxed);
        let modified = attrs.atime_mtime.map(|(_, m)| m as u64);
        items.push(Item { remote: remote.to_vec(), local, dir: false, size, permissions: None, modified });
        return Ok(());
    }
    items.push(Item {
        remote: remote.to_vec(),
        local: local.clone(),
        dir: true,
        size: 0,
        permissions: None,
        modified: None,
    });
    for entry in sftp.read_dir(remote)? {
        let path = join(remote, &entry.name);
        // links: what they point to (a link to a folder is not followed,
        // so a link loop can't make it endless)
        let attrs = if entry.attrs.is_symlink() {
            match sftp.stat(&path) {
                Ok(a) if a.is_dir() => continue,
                Ok(a) => a,
                Err(_) => continue, // a dangling link
            }
        } else {
            entry.attrs
        };
        walk_remote(sftp, names, &path, &attrs, local.join(local_name(names, &entry.name)), items, progress)?;
    }
    Ok(())
}

/// What downloading each server's file or folder to its own local path
/// involves (a synchronization's copies).
pub fn plan_download_pairs(
    sftp: &Session,
    names: &Names,
    pairs: &[(Vec<u8>, Attrs, PathBuf)],
    progress: &Progress,
) -> Result<Vec<Item>> {
    let mut items = Vec::new();
    for (remote, attrs, local) in pairs {
        walk_remote(sftp, names, remote, attrs, local.clone(), &mut items, progress)?;
    }
    Ok(items)
}

/// What uploading each local file or folder to its own server's path
/// involves.
pub fn plan_upload_pairs(names: &Names, pairs: &[(PathBuf, Vec<u8>)], progress: &Progress) -> Result<Vec<Item>> {
    let mut items = Vec::new();
    for (local, remote) in pairs {
        walk_local(names, local, remote.clone(), &mut items, progress)?;
    }
    Ok(items)
}

/// What uploading local files and folders into the remote folder `into`
/// involves.
pub fn plan_upload(names: &Names, locals: &[PathBuf], into: &[u8], progress: &Progress) -> Result<Vec<Item>> {
    let mut items = Vec::new();
    for local in locals {
        let remote = join(into, &remote_name(names, local)?);
        walk_local(names, local, remote, &mut items, progress)?;
    }
    Ok(items)
}

fn walk_local(names: &Names, local: &Path, remote: Vec<u8>, items: &mut Vec<Item>, progress: &Progress) -> Result<()> {
    if progress.stopped() {
        return Err(Error::Cancelled);
    }
    let meta = std::fs::metadata(local)?;
    if !meta.is_dir() {
        progress.total.fetch_add(meta.len(), Ordering::Relaxed);
        items.push(Item {
            remote,
            local: local.to_path_buf(),
            dir: false,
            size: meta.len(),
            permissions: Some(0o644),
            modified: meta.modified().ok().and_then(unix_secs),
        });
        return Ok(());
    }
    items.push(Item {
        remote: remote.clone(),
        local: local.to_path_buf(),
        dir: true,
        size: 0,
        permissions: Some(0o755),
        modified: None,
    });
    let mut children: Vec<PathBuf> = std::fs::read_dir(local)?.filter_map(|e| e.ok().map(|e| e.path())).collect();
    children.sort();
    for child in children {
        let name = remote_name(names, &child)?;
        walk_local(names, &child, join(&remote, &name), items, progress)?;
    }
    Ok(())
}

/// Downloads the planned items from `progress.next` on. An existing local
/// folder is used; an existing file is replaced once its copy is
/// complete.
pub fn download(sftp: &Session, names: &Names, items: &[Item], progress: &Progress) -> Result<()> {
    let start = progress.restart(items);
    for (i, item) in items.iter().enumerate().skip(start) {
        if progress.stopped() {
            return Err(Error::Cancelled);
        }
        if item.dir {
            std::fs::create_dir_all(&item.local)?;
            progress.next.store(i + 1, Ordering::Relaxed);
            continue;
        }
        if let Some(parent) = item.local.parent() {
            std::fs::create_dir_all(parent)?;
        }
        progress.set_current(names.decode(&item.remote));
        let part = local_part(&item.local);
        let offset = local_resume(&part, item);
        let base = progress.done.load(Ordering::Relaxed) + offset;
        progress.done.store(base, Ordering::Relaxed);
        progress.skipped.fetch_add(offset, Ordering::Relaxed);
        let result = sftp.download_from(&item.remote, &part, offset, &mut |done| {
            progress.done.store(base + done, Ordering::Relaxed);
            !progress.stopped()
        });
        let done = match result {
            Ok(done) => done,
            Err(e) => {
                // cancelled: nothing is left behind; paused or broken off:
                // the partial file is continued next time
                if progress.cancelled() {
                    let _ = std::fs::remove_file(&part);
                }
                return Err(e);
            }
        };
        if let Some(m) = item.modified {
            // the server's modification time kept (compare / sync go by it)
            let file = std::fs::OpenOptions::new().write(true).open(&part)?;
            file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(m))?;
        }
        std::fs::rename(&part, &item.local)?;
        progress.done.store(base + done, Ordering::Relaxed);
        progress.next.store(i + 1, Ordering::Relaxed);
    }
    Ok(())
}

/// Uploads the planned items from `progress.next` on. An existing remote
/// folder is used; an existing file is replaced once its copy is complete
/// (keeping its permissions).
pub fn upload(sftp: &Session, names: &Names, items: &[Item], progress: &Progress) -> Result<()> {
    let start = progress.restart(items);
    for (i, item) in items.iter().enumerate().skip(start) {
        if progress.stopped() {
            return Err(Error::Cancelled);
        }
        if item.dir {
            match sftp.mkdir(&item.remote) {
                Ok(()) => {}
                // already there (as a folder: stat tells)
                Err(e) => {
                    if !sftp.stat(&item.remote).is_ok_and(|a| a.is_dir()) {
                        return Err(e);
                    }
                }
            }
            progress.next.store(i + 1, Ordering::Relaxed);
            continue;
        }
        progress.set_current(names.decode(&item.remote));
        let part = remote_part(&item.remote);
        let offset = remote_resume(sftp, &part, item);
        let base = progress.done.load(Ordering::Relaxed) + offset;
        progress.done.store(base, Ordering::Relaxed);
        progress.skipped.fetch_add(offset, Ordering::Relaxed);
        let result = sftp.upload_from(&item.local, &part, offset, item.permissions, &mut |done| {
            progress.done.store(base + done, Ordering::Relaxed);
            !progress.stopped()
        });
        let done = match result {
            Ok(done) => done,
            Err(e) => {
                if progress.cancelled() {
                    let _ = sftp.remove(&part);
                }
                return Err(e);
            }
        };
        if let Some(m) = item.modified.and_then(|m| u32::try_from(m).ok()) {
            // the local modification time kept (compare / sync go by it)
            let _ = sftp.setstat(&part, &Attrs { atime_mtime: Some((m, m)), ..Default::default() });
        }
        finish_upload(sftp, &part, &item.remote)?;
        progress.done.store(base + done, Ordering::Relaxed);
        progress.next.store(i + 1, Ordering::Relaxed);
    }
    Ok(())
}

/// Removes the partial file of the item a paused or failed transfer
/// stopped at (when it is cancelled for good).
pub fn discard(sftp: Option<&Session>, items: &[Item], progress: &Progress, upload: bool) {
    let Some(item) = items.get(progress.next.load(Ordering::Relaxed)).filter(|i| !i.dir) else { return };
    if upload {
        if let Some(sftp) = sftp {
            let _ = sftp.remove(&remote_part(&item.remote));
        }
    } else {
        let _ = std::fs::remove_file(local_part(&item.local));
    }
}

/// The complete upload in the file's place: renamed over it (with its
/// permissions), or, where the server can't rename over a file, after it
/// is removed.
fn finish_upload(sftp: &Session, part: &[u8], remote: &[u8]) -> Result<()> {
    if let Ok(old) = sftp.stat(remote) {
        if let Some(p) = old.permissions {
            let _ = sftp.setstat(part, &Attrs { permissions: Some(p & 0o7777), ..Default::default() });
        }
    }
    if sftp.rename(part, remote, true).is_ok() {
        return Ok(());
    }
    let _ = sftp.remove(remote);
    sftp.rename(part, remote, false)
}

/// A local file's name while it is being downloaded.
pub fn local_part(local: &Path) -> PathBuf {
    let mut name = local.as_os_str().to_os_string();
    name.push(PART);
    PathBuf::from(name)
}

/// A server's file's name while it is being uploaded.
pub fn remote_part(remote: &[u8]) -> Vec<u8> {
    [remote, PART.as_bytes()].concat()
}

/// Where a local partial copy goes on: its length, if it is no longer
/// than the file and not older than the file's last change (else 0).
fn local_resume(part: &Path, item: &Item) -> u64 {
    let Ok(meta) = std::fs::metadata(part) else { return 0 };
    resume_at(meta.len(), meta.modified().ok().and_then(unix_secs), item)
}

fn remote_resume(sftp: &Session, part: &[u8], item: &Item) -> u64 {
    let Ok(attrs) = sftp.stat(part) else { return 0 };
    resume_at(attrs.size.unwrap_or(0), attrs.atime_mtime.map(|(_, m)| m as u64), item)
}

fn resume_at(len: u64, written: Option<u64>, item: &Item) -> u64 {
    let changed_since = matches!((written, item.modified), (Some(w), Some(m)) if m > w);
    if len > item.size || changed_since {
        0
    } else {
        len
    }
}

fn unix_secs(time: std::time::SystemTime) -> Option<u64> {
    time.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs())
}

/// Deletes a file, a link, or a folder and everything in it (links inside
/// are removed, not followed).
pub fn remove(sftp: &Session, path: &[u8], attrs: &Attrs) -> Result<()> {
    if !attrs.is_dir() || attrs.is_symlink() {
        return sftp.remove(path);
    }
    for entry in sftp.read_dir(path)? {
        remove(sftp, &join(path, &entry.name), &entry.attrs)?;
    }
    sftp.rmdir(path)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::process::Command;

    fn local_server(dir: &Path) -> Option<Session> {
        let server = Path::new(r"C:\Windows\System32\OpenSSH\sftp-server.exe");
        if !server.exists() {
            return None;
        }
        let mut command = Command::new(server);
        command.arg("-d").arg(dir);
        Some(Session::spawn(command).unwrap())
    }

    fn remote(path: &Path) -> Vec<u8> {
        format!("/{}", path.display().to_string().replace('\\', "/")).into_bytes()
    }

    #[test]
    fn windows_names() {
        let names = Names::default();
        assert_eq!(local_name(&names, b"a:b?.txt"), "a_b_.txt");
        assert_eq!(local_name(&names, b"con.txt"), "_con.txt");
        assert_eq!(local_name(&names, b"COM3"), "_COM3");
        assert_eq!(local_name(&names, b"console.log"), "console.log");
        assert_eq!(local_name(&names, b"trailing. "), "trailing");
        assert_eq!(local_name(&names, "中文.txt".as_bytes()), "中文.txt");
        let gbk = Names::from_label("gbk").unwrap();
        assert_eq!(local_name(&gbk, &[0xd6, 0xd0, 0xce, 0xc4]), "中文");
        assert!(remote_name(&Names::from_label("windows-1251").unwrap(), Path::new("中文.txt")).is_err());
    }

    /// A folder tree up and down again, with an empty folder, nested
    /// files and CJK names; progress adds up; delete removes it all.
    #[test]
    fn folders_both_ways() {
        let dir = tempfile::tempdir().unwrap();
        let Some(sftp) = local_server(dir.path()) else { return };
        let names = Names::default();
        let src = dir.path().join("源");
        std::fs::create_dir_all(src.join("子目录").join("空")).unwrap();
        std::fs::write(src.join("a.txt"), b"alpha").unwrap();
        // an old modification time, to see it kept both ways
        let old = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_577_836_800);
        std::fs::File::options().write(true).open(src.join("a.txt")).unwrap().set_modified(old).unwrap();
        std::fs::write(src.join("子目录").join("b.bin"), vec![9u8; 200_000]).unwrap();
        let server_dir = dir.path().join("server");
        std::fs::create_dir(&server_dir).unwrap();

        let progress = Progress::default();
        let items = plan_upload(&names, std::slice::from_ref(&src), &remote(&server_dir), &progress).unwrap();
        assert_eq!(items.iter().filter(|i| i.dir).count(), 3);
        assert_eq!(progress.total.load(Ordering::Relaxed), 200_005);
        upload(&sftp, &names, &items, &progress).unwrap();
        assert_eq!(progress.done.load(Ordering::Relaxed), 200_005);
        assert_eq!(std::fs::read(server_dir.join("源").join("子目录").join("b.bin")).unwrap().len(), 200_000);
        assert!(server_dir.join("源").join("子目录").join("空").is_dir());
        // again: existing folders are used, files replaced
        upload(&sftp, &names, &items, &Progress::default()).unwrap();

        let back = dir.path().join("back");
        std::fs::create_dir(&back).unwrap();
        let remote_src = join(&remote(&server_dir), "源".as_bytes());
        let attrs = sftp.stat(&remote_src).unwrap();
        let progress = Progress::default();
        let items = plan_download(&sftp, &names, &remote_src, &attrs, &back, &progress).unwrap();
        download(&sftp, &names, &items, &progress).unwrap();
        assert_eq!(progress.done.load(Ordering::Relaxed), progress.total.load(Ordering::Relaxed));
        assert_eq!(std::fs::read(back.join("源").join("a.txt")).unwrap(), b"alpha");
        assert!(back.join("源").join("子目录").join("空").is_dir());
        let up = sftp.stat(&join(&remote_src, b"a.txt")).unwrap();
        assert_eq!(up.atime_mtime.map(|(_, m)| m), Some(1_577_836_800));
        assert_eq!(std::fs::metadata(back.join("源").join("a.txt")).unwrap().modified().unwrap(), old);

        remove(&sftp, &remote_src, &attrs).unwrap();
        assert!(!server_dir.join("源").exists());
    }

    #[test]
    fn a_cancelled_download_leaves_no_partial_file() {
        let dir = tempfile::tempdir().unwrap();
        let Some(sftp) = local_server(dir.path()) else { return };
        let names = Names::default();
        std::fs::write(dir.path().join("big.bin"), vec![3u8; 16 * 1024 * 1024]).unwrap();
        let out = dir.path().join("out");
        std::fs::create_dir(&out).unwrap();
        let path = join(&remote(dir.path()), b"big.bin");
        let attrs = sftp.stat(&path).unwrap();
        let progress = std::sync::Arc::new(Progress::default());
        let items = plan_download(&sftp, &names, &path, &attrs, &out, &progress).unwrap();
        let p = std::sync::Arc::clone(&progress);
        let watcher = std::thread::spawn(move || {
            while p.done.load(Ordering::Relaxed) < 1024 * 1024 {
                std::thread::yield_now();
            }
            p.cancel.store(true, Ordering::Relaxed);
        });
        assert_eq!(download(&sftp, &names, &items, &progress), Err(Error::Cancelled));
        watcher.join().unwrap();
        assert!(!out.join("big.bin").exists());
        assert!(!local_part(&out.join("big.bin")).exists());
    }

    /// Pauses once `at` bytes are done (from another thread).
    fn pause_at(progress: &std::sync::Arc<Progress>, at: u64) -> std::thread::JoinHandle<()> {
        let p = std::sync::Arc::clone(progress);
        std::thread::spawn(move || {
            while p.done.load(Ordering::Relaxed) < at {
                std::thread::yield_now();
            }
            p.pause.store(true, Ordering::Relaxed);
        })
    }

    fn pattern(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 7 % 251) as u8).collect()
    }

    /// Paused halfway, a download leaves a partial file (not the real
    /// name); going on with the same plan continues it to the same bytes.
    #[test]
    fn a_paused_download_goes_on_where_it_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let Some(sftp) = local_server(dir.path()) else { return };
        let names = Names::default();
        let data = pattern(12 * 1024 * 1024);
        std::fs::write(dir.path().join("a.bin"), &data).unwrap();
        std::fs::write(dir.path().join("b.txt"), b"second").unwrap();
        let out = dir.path().join("out");
        std::fs::create_dir(&out).unwrap();
        let progress = std::sync::Arc::new(Progress::default());
        let mut items = Vec::new();
        for name in ["a.bin", "b.txt"] {
            let path = join(&remote(dir.path()), name.as_bytes());
            let attrs = sftp.stat(&path).unwrap();
            items.extend(plan_download(&sftp, &names, &path, &attrs, &out, &progress).unwrap());
        }
        let watcher = pause_at(&progress, 4 * 1024 * 1024);
        assert_eq!(download(&sftp, &names, &items, &progress), Err(Error::Cancelled));
        watcher.join().unwrap();
        let part = local_part(&out.join("a.bin"));
        let kept = std::fs::metadata(&part).unwrap().len();
        assert!(kept >= 4 * 1024 * 1024 && kept < data.len() as u64, "{kept}");
        assert!(!out.join("a.bin").exists());
        assert_eq!(progress.next.load(Ordering::Relaxed), 0);

        progress.pause.store(false, Ordering::Relaxed);
        download(&sftp, &names, &items, &progress).unwrap();
        assert_eq!(std::fs::read(out.join("a.bin")).unwrap(), data);
        assert_eq!(std::fs::read(out.join("b.txt")).unwrap(), b"second");
        assert!(!part.exists());
        assert_eq!(progress.done.load(Ordering::Relaxed), progress.total.load(Ordering::Relaxed));
        assert_eq!(progress.next.load(Ordering::Relaxed), items.len());
    }

    /// The same for an upload: the partial file on the server is
    /// continued, then renamed over the old file.
    #[test]
    fn a_paused_upload_goes_on_where_it_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let Some(sftp) = local_server(dir.path()) else { return };
        let names = Names::default();
        let data = pattern(12 * 1024 * 1024);
        let src = dir.path().join("up.bin");
        std::fs::write(&src, &data).unwrap();
        let server_dir = dir.path().join("server");
        std::fs::create_dir(&server_dir).unwrap();
        std::fs::write(server_dir.join("up.bin"), b"old").unwrap();
        let progress = std::sync::Arc::new(Progress::default());
        let items = plan_upload(&names, std::slice::from_ref(&src), &remote(&server_dir), &progress).unwrap();
        let watcher = pause_at(&progress, 4 * 1024 * 1024);
        assert_eq!(upload(&sftp, &names, &items, &progress), Err(Error::Cancelled));
        watcher.join().unwrap();
        // asked through the session: the writes still in flight when it
        // paused are done first (the server handles requests in order)
        let remote_file = join(&remote(&server_dir), b"up.bin");
        let kept = sftp.stat(&remote_part(&remote_file)).unwrap().size.unwrap();
        assert!(kept > 0 && kept < data.len() as u64, "{kept}");
        assert_eq!(std::fs::read(server_dir.join("up.bin")).unwrap(), b"old");
        let part = server_dir.join(format!("up.bin{PART}"));

        // a new transfer of the same file (the window closed meanwhile)
        // continues the partial file too
        let again = Progress::default();
        let items = plan_upload(&names, std::slice::from_ref(&src), &remote(&server_dir), &again).unwrap();
        assert_eq!(remote_resume(&sftp, &remote_part(&remote_file), &items[0]), kept);
        upload(&sftp, &names, &items, &again).unwrap();
        assert_eq!(std::fs::read(server_dir.join("up.bin")).unwrap(), data);
        assert!(!part.exists());
    }

    /// A partial file is only continued if it can be part of the file as
    /// it is now.
    #[test]
    fn when_a_partial_file_is_continued() {
        let item = Item {
            remote: b"/f".to_vec(),
            local: PathBuf::from("f"),
            dir: false,
            size: 100,
            permissions: None,
            modified: Some(1_000),
        };
        assert_eq!(resume_at(40, Some(2_000), &item), 40);
        assert_eq!(resume_at(100, Some(2_000), &item), 100);
        // longer than the file, or older than its last change: again from 0
        assert_eq!(resume_at(140, Some(2_000), &item), 0);
        assert_eq!(resume_at(40, Some(500), &item), 0);
        // times unknown: the length decides
        assert_eq!(resume_at(40, None, &item), 40);
    }
}
