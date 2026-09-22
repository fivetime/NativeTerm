//! Which process a pid is: when it started (ids are reused, start times
//! are not), who started it, what program it runs.

#[cfg(windows)]
pub use native_term_win::desktop::process_image as image;
#[cfg(windows)]
pub use native_term_win::{parent_pid, process_started as started};

#[cfg(target_os = "linux")]
mod unix {
    use std::path::PathBuf;

    /// Field `n` (1-based, as `proc(5)` counts them) of `/proc/<pid>/stat`;
    /// the command name in parentheses may hold spaces, so the fields
    /// after it are counted from its closing parenthesis.
    fn stat_field(pid: u32, n: usize) -> Option<String> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let after = &stat[stat.rfind(')')? + 1..];
        after.split_whitespace().nth(n - 3).map(str::to_string)
    }

    /// When a running process started, in clock ticks since boot: with
    /// the id, one process.
    pub fn started(pid: u32) -> Option<u64> {
        stat_field(pid, 22)?.parse().ok()
    }

    pub fn parent_pid(pid: u32) -> Option<u32> {
        stat_field(pid, 4)?.parse().ok()
    }

    pub fn image(pid: u32) -> Option<PathBuf> {
        std::fs::read_link(format!("/proc/{pid}/exe")).ok()
    }
}

#[cfg(target_os = "macos")]
mod unix {
    use std::path::PathBuf;

    fn bsd_info(pid: u32) -> Option<libc::proc_bsdinfo> {
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
        // SAFETY: `info` is a zeroed struct of the size passed; the call
        // fills it and returns how many bytes it wrote (0 on failure).
        let got = unsafe {
            libc::proc_pidinfo(
                pid as i32,
                libc::PROC_PIDTBSDINFO,
                0,
                (&mut info as *mut libc::proc_bsdinfo).cast(),
                size,
            )
        };
        (got == size).then_some(info)
    }

    /// When a running process started, in microseconds since the epoch:
    /// with the id, one process.
    pub fn started(pid: u32) -> Option<u64> {
        let info = bsd_info(pid)?;
        Some(info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec)
    }

    pub fn parent_pid(pid: u32) -> Option<u32> {
        Some(bsd_info(pid)?.pbi_ppid)
    }

    pub fn image(pid: u32) -> Option<PathBuf> {
        let mut buf = [0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        // SAFETY: the buffer and its length go together; the call writes
        // at most that many bytes and returns how many (0 on failure).
        let len = unsafe { libc::proc_pidpath(pid as i32, buf.as_mut_ptr().cast(), buf.len() as u32) };
        (len > 0).then(|| PathBuf::from(String::from_utf8_lossy(&buf[..len as usize]).into_owned()))
    }
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
mod unix {
    use std::path::PathBuf;

    pub fn started(_pid: u32) -> Option<u64> {
        None
    }

    pub fn parent_pid(_pid: u32) -> Option<u32> {
        None
    }

    pub fn image(_pid: u32) -> Option<PathBuf> {
        None
    }
}

#[cfg(unix)]
pub use unix::{image, parent_pid, started};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process() {
        let me = std::process::id();
        assert!(started(me).is_some());
        let image = image(me).expect("our own image");
        assert!(image.is_absolute(), "{}", image.display());
        let parent = parent_pid(me).expect("a parent");
        assert_ne!(parent, me);
    }
}
