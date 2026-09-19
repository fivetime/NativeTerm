//! Folder change notifications (`FindFirstChangeNotificationW`): a thread
//! that sleeps until something in the folder changes, then calls back.
//! No polling. Dropping the watcher stops the thread.

use std::io;
use std::path::Path;
use std::thread::JoinHandle;

use windows::core::HSTRING;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Storage::FileSystem::{
    FindCloseChangeNotification, FindFirstChangeNotificationW, FindNextChangeNotification, FILE_NOTIFY_CHANGE_DIR_NAME,
    FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE,
};
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForMultipleObjects, INFINITE};

/// Handles are plain kernel handles, usable from any thread.
struct Handle(HANDLE);
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}

pub struct FolderWatcher {
    stop: Handle,
    thread: Option<JoinHandle<()>>,
}

impl FolderWatcher {
    /// Watch `dir` (and its subfolders if `subtree`); `changed` runs on the
    /// watcher's thread after each burst of changes.
    pub fn start(dir: &Path, subtree: bool, changed: impl Fn() + Send + 'static) -> io::Result<FolderWatcher> {
        let filter = FILE_NOTIFY_CHANGE_FILE_NAME
            | FILE_NOTIFY_CHANGE_DIR_NAME
            | FILE_NOTIFY_CHANGE_LAST_WRITE
            | FILE_NOTIFY_CHANGE_SIZE;
        let notification = unsafe { FindFirstChangeNotificationW(&HSTRING::from(dir), subtree, filter)? };
        let stop = unsafe { CreateEventW(None, true, false, None)? };
        let (notification, stop_copy) = (Handle(notification), Handle(stop));
        let thread = std::thread::Builder::new().name("folder-watcher".into()).spawn(move || {
            let (notification, stop_copy) = (notification, stop_copy);
            let handles = [stop_copy.0, notification.0];
            loop {
                let result = unsafe { WaitForMultipleObjects(&handles, false, INFINITE) };
                if result != windows::Win32::Foundation::WAIT_EVENT(WAIT_OBJECT_0.0 + 1) {
                    break;
                }
                // let a save (write, rename, attribute change) finish
                std::thread::sleep(std::time::Duration::from_millis(150));
                changed();
                if unsafe { FindNextChangeNotification(notification.0) }.is_err() {
                    break;
                }
            }
            unsafe {
                let _ = FindCloseChangeNotification(notification.0);
            }
        })?;
        Ok(FolderWatcher { stop: Handle(stop), thread: Some(thread) })
    }
}

impl Drop for FolderWatcher {
    fn drop(&mut self) {
        unsafe {
            let _ = SetEvent(self.stop.0);
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        unsafe {
            let _ = CloseHandle(self.stop.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    #[test]
    fn reports_changes_and_stops() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("config.d")).unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&count);
        let watcher = FolderWatcher::start(dir.path(), true, move || {
            seen.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
        std::fs::write(dir.path().join("config.d").join("a.conf"), "Host a\n").unwrap();
        let started = Instant::now();
        while count.load(Ordering::SeqCst) == 0 {
            assert!(started.elapsed() < Duration::from_secs(5), "no notification");
            std::thread::sleep(Duration::from_millis(20));
        }
        drop(watcher);
        let after = count.load(Ordering::SeqCst);
        std::fs::write(dir.path().join("b.txt"), "x").unwrap();
        std::thread::sleep(Duration::from_millis(400));
        assert_eq!(count.load(Ordering::SeqCst), after, "stopped");
    }
}
