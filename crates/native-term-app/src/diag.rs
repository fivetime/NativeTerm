//! A small local log for troubleshooting, in `logs\` of the data
//! directory: one file per day, pruned after a fortnight. Nothing leaves
//! the machine (see "Diagnostics, no telemetry" in `docs/ARCHITECTURE.md`).
//!
//! What goes in: the starts and stops, and every notice the user is shown.
//! Notices are written for people to read, so they hold no secrets; a
//! password, a key or the text of a sent command never reaches this file
//! (sent commands have their own, `audit\`, which the user can delete).
//!
//! Each line is opened, written and closed: a log that is never held open
//! survives a data directory on a synced or removable drive, and a few
//! lines a minute cost nothing.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Files older than this are removed at start.
pub const KEEP_DAYS: i64 = 14;
/// A day's file stops growing here (a loop of notices can't fill a disk).
pub const MAX_BYTES: u64 = 8 * 1024 * 1024;

static SINK: Mutex<Option<PathBuf>> = Mutex::new(None);

fn sink() -> std::sync::MutexGuard<'static, Option<PathBuf>> {
    SINK.lock().unwrap_or_else(|e| e.into_inner())
}

/// The file a line written now belongs in.
fn file_for(dir: &Path, date: &str) -> PathBuf {
    dir.join(format!("nativeterm-{date}.log"))
}

/// Start logging into `dir` (created if needed) and drop what is too old.
/// A directory that can't be written is simply not logged to.
pub fn open(dir: &Path) {
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    *sink() = Some(dir.to_path_buf());
    prune(dir, crate::utc_now().0.as_str());
    line(&format!("NativeTerm {} started", env!("CARGO_PKG_VERSION")));
}

/// Stop logging (the last line first).
pub fn close(why: &str) {
    line(why);
    *sink() = None;
}

/// One line, with the time in UTC. Never fails: a log that can't be
/// written must not disturb anything.
pub fn line(text: &str) {
    let Some(dir) = sink().clone() else { return };
    let (date, time) = crate::utc_now();
    let path = file_for(&dir, &date);
    if std::fs::metadata(&path).is_ok_and(|m| m.len() >= MAX_BYTES) {
        return;
    }
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        // one line, whatever the text was
        let text = text.replace("\r\n", "\n").replace(['\r', '\n'], " ");
        let _ = writeln!(file, "{date} {time} {text}");
    }
}

/// Days between two `YYYY-MM-DD` dates, or `None` if either isn't one.
fn days_between(from: &str, to: &str) -> Option<i64> {
    Some(day_number(to)? - day_number(from)?)
}

/// A date as a day count (only differences are used, so any epoch does).
fn day_number(date: &str) -> Option<i64> {
    let mut parts = date.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: i64 = parts.next()?.parse().ok()?;
    let day: i64 = parts.next()?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // civil to days (Howard Hinnant's algorithm)
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

/// Our own log files older than `KEEP_DAYS`.
fn stale(dir: &Path, today: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            let date = name.strip_prefix("nativeterm-")?.strip_suffix(".log")?;
            (days_between(date, today)? > KEEP_DAYS).then_some(path)
        })
        .collect()
}

fn prune(dir: &Path, today: &str) {
    for path in stale(dir, today) {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_lands_in_todays_file_and_stays_one_line() {
        let tmp = tempfile::tempdir().unwrap();
        open(tmp.path());
        line("two\r\nlines");
        close("done");
        let date = crate::utc_now().0;
        let text = std::fs::read_to_string(file_for(tmp.path(), &date)).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "{text}");
        assert!(lines[0].contains("started"), "{text}");
        assert!(lines[1].ends_with("two lines"), "{text}");
        assert!(lines[1].starts_with(&date), "{text}");
        assert!(lines[2].ends_with("done"), "{text}");
        // closed: nothing more is written
        line("after");
        assert_eq!(std::fs::read_to_string(file_for(tmp.path(), &date)).unwrap(), text);
    }

    #[test]
    fn old_files_go_and_recent_ones_stay() {
        let tmp = tempfile::tempdir().unwrap();
        for date in ["2026-09-01", "2026-09-20", "2026-09-21"] {
            std::fs::write(file_for(tmp.path(), date), "x").unwrap();
        }
        // not ours
        std::fs::write(tmp.path().join("commands-2026-01.log"), "x").unwrap();
        std::fs::write(tmp.path().join("nativeterm-notes.log"), "x").unwrap();
        prune(tmp.path(), "2026-09-21");
        let left: Vec<String> =
            std::fs::read_dir(tmp.path()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into()).collect();
        assert!(!left.contains(&"nativeterm-2026-09-01.log".to_string()), "{left:?}");
        assert_eq!(left.len(), 4, "{left:?}");
    }

    #[test]
    fn dates_are_counted_across_months_and_years() {
        assert_eq!(days_between("2026-09-01", "2026-09-21"), Some(20));
        assert_eq!(days_between("2025-12-31", "2026-01-01"), Some(1));
        assert_eq!(days_between("2024-02-28", "2024-03-01"), Some(2), "a leap year");
        assert_eq!(days_between("2026-09-21", "2026-09-01"), Some(-20));
        assert_eq!(days_between("nativeterm", "2026-09-21"), None);
        assert_eq!(days_between("2026-13-01", "2026-09-21"), None);
    }
}
