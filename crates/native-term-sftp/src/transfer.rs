//! Transfers of files and folders: first a plan (every file and folder,
//! and the total size, so progress is a fraction), then the copying, with
//! progress and cancelling. Also deleting a folder with what's in it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
}

/// A transfer's state, shared with whoever shows it.
#[derive(Default)]
pub struct Progress {
    pub done: AtomicU64,
    pub total: AtomicU64,
    pub cancel: AtomicBool,
    /// The file being copied (for showing).
    pub current: Mutex<String>,
}

impl Progress {
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
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
    let mut out: String =
        text.chars().map(|c| if c < ' ' || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') { '_' } else { c }).collect();
    while out.ends_with(['.', ' ']) {
        out.pop();
    }
    let stem = out.split('.').next().unwrap_or("").to_ascii_uppercase();
    let device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && stem.as_bytes()[3].is_ascii_digit());
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
    names.encode(&name).ok_or_else(|| Error::Protocol(format!("\"{name}\" can't be written in the server's file name encoding")))
}

/// What downloading `remote` (a file or, with `attrs.is_dir()`, a folder
/// and everything below) into the folder `into` involves.
pub fn plan_download(sftp: &Session, names: &Names, remote: &[u8], attrs: &Attrs, into: &Path, progress: &Progress) -> Result<Vec<Item>> {
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
    if progress.cancelled() {
        return Err(Error::Cancelled);
    }
    if !attrs.is_dir() {
        let size = attrs.size.unwrap_or(0);
        progress.total.fetch_add(size, Ordering::Relaxed);
        items.push(Item { remote: remote.to_vec(), local, dir: false, size, permissions: None });
        return Ok(());
    }
    items.push(Item { remote: remote.to_vec(), local: local.clone(), dir: true, size: 0, permissions: None });
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
    if progress.cancelled() {
        return Err(Error::Cancelled);
    }
    let meta = std::fs::metadata(local)?;
    if !meta.is_dir() {
        progress.total.fetch_add(meta.len(), Ordering::Relaxed);
        items.push(Item { remote, local: local.to_path_buf(), dir: false, size: meta.len(), permissions: Some(0o644) });
        return Ok(());
    }
    items.push(Item { remote: remote.clone(), local: local.to_path_buf(), dir: true, size: 0, permissions: Some(0o755) });
    let mut children: Vec<PathBuf> = std::fs::read_dir(local)?.filter_map(|e| e.ok().map(|e| e.path())).collect();
    children.sort();
    for child in children {
        let name = remote_name(names, &child)?;
        walk_local(names, &child, join(&remote, &name), items, progress)?;
    }
    Ok(())
}

/// Downloads the planned items. An existing local folder is used; an
/// existing file is replaced.
pub fn download(sftp: &Session, names: &Names, items: &[Item], progress: &Progress) -> Result<()> {
    for item in items {
        if progress.cancelled() {
            return Err(Error::Cancelled);
        }
        if item.dir {
            std::fs::create_dir_all(&item.local)?;
            continue;
        }
        if let Some(parent) = item.local.parent() {
            std::fs::create_dir_all(parent)?;
        }
        progress.set_current(names.decode(&item.remote));
        let base = progress.done.load(Ordering::Relaxed);
        let result = sftp.download(&item.remote, &item.local, &mut |done| {
            progress.done.store(base + done, Ordering::Relaxed);
            !progress.cancelled()
        });
        if result.is_err() {
            // a partial file isn't left behind
            let _ = std::fs::remove_file(&item.local);
        }
        let done = result?;
        progress.done.store(base + done, Ordering::Relaxed);
    }
    Ok(())
}

/// Uploads the planned items. An existing remote folder is used; an
/// existing file is replaced.
pub fn upload(sftp: &Session, names: &Names, items: &[Item], progress: &Progress) -> Result<()> {
    for item in items {
        if progress.cancelled() {
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
            continue;
        }
        progress.set_current(names.decode(&item.remote));
        let base = progress.done.load(Ordering::Relaxed);
        let done = sftp.upload(&item.local, &item.remote, item.permissions, &mut |done| {
            progress.done.store(base + done, Ordering::Relaxed);
            !progress.cancelled()
        })?;
        progress.done.store(base + done, Ordering::Relaxed);
    }
    Ok(())
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
    }
}
