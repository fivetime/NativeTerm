//! Folder change notifications: a thread that sleeps until something in
//! the folder changes, then calls back. No polling. Dropping the watcher
//! stops it.

#[cfg(windows)]
pub use native_term_win::watch::FolderWatcher;

#[cfg(unix)]
mod unix {
    use std::io;
    use std::path::Path;

    use notify::{RecursiveMode, Watcher};

    /// Kept alive for as long as the folder is watched.
    pub struct FolderWatcher {
        _watcher: notify::RecommendedWatcher,
    }

    impl FolderWatcher {
        /// Watch `dir` (and its subfolders if `subtree`); `changed` runs on
        /// the watcher's thread after each change.
        pub fn start(dir: &Path, subtree: bool, changed: impl Fn() + Send + 'static) -> io::Result<FolderWatcher> {
            let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                // a change, not a look: inotify also reports files being
                // opened and read, and the program reads the folder when it
                // is told of a change, which would tell it again, forever
                // (a frame per display refresh on Linux, 30 % of a CPU)
                if event.is_ok_and(|e| super::is_change(&e.kind)) {
                    changed();
                }
            })
            .map_err(io::Error::other)?;
            let mode = if subtree { RecursiveMode::Recursive } else { RecursiveMode::NonRecursive };
            watcher.watch(dir, mode).map_err(io::Error::other)?;
            Ok(FolderWatcher { _watcher: watcher })
        }
    }
}

#[cfg(unix)]
pub use unix::FolderWatcher;

/// Whether an event changes the folder (created, written, renamed,
/// removed), rather than only looking at it (opened, read, closed
/// without writing).
#[cfg(unix)]
fn is_change(kind: &notify::EventKind) -> bool {
    use notify::event::{AccessKind, AccessMode, EventKind};
    match kind {
        EventKind::Access(AccessKind::Close(AccessMode::Write)) => true,
        EventKind::Access(_) | EventKind::Any | EventKind::Other => false,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => true,
    }
}

#[cfg(all(test, unix))]
mod kinds {
    use notify::event::{AccessKind, AccessMode, CreateKind, DataChange, EventKind, ModifyKind, RemoveKind};

    #[test]
    fn reading_is_no_change() {
        assert!(!super::is_change(&EventKind::Access(AccessKind::Open(AccessMode::Read))));
        assert!(!super::is_change(&EventKind::Access(AccessKind::Close(AccessMode::Read))));
        assert!(!super::is_change(&EventKind::Access(AccessKind::Read)));
        assert!(super::is_change(&EventKind::Access(AccessKind::Close(AccessMode::Write))));
        assert!(super::is_change(&EventKind::Create(CreateKind::File)));
        assert!(super::is_change(&EventKind::Modify(ModifyKind::Data(DataChange::Any))));
        assert!(super::is_change(&EventKind::Remove(RemoveKind::File)));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    #[test]
    fn a_change_is_noticed() {
        let dir = tempfile::tempdir().unwrap();
        let seen = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&seen);
        let watcher = super::FolderWatcher::start(dir.path(), false, move || {
            count.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
        std::thread::sleep(Duration::from_millis(200));
        std::fs::write(dir.path().join("a.txt"), b"x").unwrap();
        let started = Instant::now();
        while seen.load(Ordering::SeqCst) == 0 {
            assert!(started.elapsed() < Duration::from_secs(10), "no notification");
            std::thread::sleep(Duration::from_millis(50));
        }
        drop(watcher);
    }
}
