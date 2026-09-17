//! Writing ssh config files safely. Every ssh tool on the machine reads
//! these files, so a write:
//!
//! 1. refuses if the file changed since it was read ([`Fingerprint`]);
//! 2. backs up the previous version (timestamped, pruned by count and age);
//! 3. replaces the file atomically (temp file + `ReplaceFileW`, which keeps
//!    the existing ACL; new files get an owner-only ACL, as ssh requires);
//! 4. runs a validation (normally `ssh -G`) and restores the previous
//!    version if it fails.

use std::fmt;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::document::Document;

/// What a file looked like when it was read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fingerprint {
    len: u64,
    modified: Option<SystemTime>,
    hash: u64,
}

impl Fingerprint {
    /// `None` if the file doesn't exist.
    pub fn of(path: &Path) -> io::Result<Option<Fingerprint>> {
        match fs::read(path) {
            Ok(bytes) => Ok(Some(Self::from_bytes(path, &bytes)?)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn from_bytes(path: &Path, bytes: &[u8]) -> io::Result<Fingerprint> {
        let mut hasher = DefaultHasher::new();
        bytes.hash(&mut hasher);
        Ok(Fingerprint {
            len: bytes.len() as u64,
            modified: fs::metadata(path)?.modified().ok(),
            hash: hasher.finish(),
        })
    }
}

/// Read a file for editing; a missing file reads as empty.
pub fn read(path: &Path) -> io::Result<(String, Option<Fingerprint>)> {
    match fs::read(path) {
        Ok(bytes) => {
            let fingerprint = Fingerprint::from_bytes(path, &bytes)?;
            let text = String::from_utf8(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            Ok((text, Some(fingerprint)))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok((String::new(), None)),
        Err(e) => Err(e),
    }
}

#[derive(Debug)]
pub enum WriteError {
    /// The file changed since it was read; nothing was written.
    Conflict,
    /// Validation rejected the new content; the previous content is back.
    Rejected { reason: String, backup: Option<PathBuf> },
    Io(io::Error),
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WriteError::Conflict => write!(f, "the file was changed by someone else since it was read"),
            WriteError::Rejected { reason, .. } => write!(f, "ssh rejected the change, previous version restored: {reason}"),
            WriteError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for WriteError {}

impl From<io::Error> for WriteError {
    fn from(e: io::Error) -> Self {
        WriteError::Io(e)
    }
}

pub struct Writer {
    /// Normally `<data dir>\backups`.
    pub backups: PathBuf,
    /// Backups kept per file.
    pub keep: usize,
    /// Older backups are removed regardless of count.
    pub max_age: Duration,
}

impl Writer {
    pub fn new(backups: impl Into<PathBuf>) -> Writer {
        Writer { backups: backups.into(), keep: 50, max_age: Duration::from_secs(30 * 24 * 3600) }
    }

    /// Replace `path` with `text` if it still matches `expected` (`None`:
    /// the file must not exist). Returns the backup of the previous version.
    pub fn write(
        &self,
        path: &Path,
        text: &str,
        expected: Option<Fingerprint>,
        validate: impl FnOnce() -> Result<(), String>,
    ) -> Result<Option<PathBuf>, WriteError> {
        let previous = match fs::read(path) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        let current = match &previous {
            Some(bytes) => Some(Fingerprint::from_bytes(path, bytes)?),
            None => None,
        };
        if current != expected {
            return Err(WriteError::Conflict);
        }
        let backup = match &previous {
            Some(bytes) => Some(self.backup(path, bytes)?),
            None => None,
        };
        replace(path, text.as_bytes())?;
        if let Err(reason) = validate() {
            match &previous {
                Some(bytes) => replace(path, bytes)?,
                None => fs::remove_file(path)?,
            }
            return Err(WriteError::Rejected { reason, backup });
        }
        Ok(backup)
    }

    fn backup(&self, path: &Path, bytes: &[u8]) -> io::Result<PathBuf> {
        let dir = self.backups.join(backup_group(path));
        fs::create_dir_all(&dir)?;
        // a zero-padded counter keeps names sortable within one millisecond
        let stamp = timestamp(SystemTime::now());
        let file = (1..)
            .map(|n| dir.join(format!("{stamp}-{n:03}.bak")))
            .find(|f| !f.exists())
            .expect("unbounded range");
        fs::write(&file, bytes)?;
        // backups hold host details too
        #[cfg(windows)]
        crate::acl::restrict_to_owner(&file)?;
        self.prune(&dir)?;
        Ok(file)
    }

    fn prune(&self, dir: &Path) -> io::Result<()> {
        let mut files: Vec<(String, PathBuf)> = fs::read_dir(dir)?
            .filter_map(Result::ok)
            .map(|e| (e.file_name().to_string_lossy().to_string(), e.path()))
            .filter(|(name, _)| name.ends_with(".bak"))
            .collect();
        // names start with a sortable timestamp: newest first
        files.sort_by(|a, b| b.0.cmp(&a.0));
        let now = SystemTime::now();
        for (index, (_, path)) in files.iter().enumerate() {
            let too_old = fs::metadata(path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| now.duration_since(t).ok())
                .is_some_and(|age| age > self.max_age);
            if index >= self.keep || too_old {
                fs::remove_file(path)?;
            }
        }
        Ok(())
    }
}

/// Read, change, and safely write one config file. Returns `false` if the
/// change left the text as it was (nothing written).
pub fn edit_file(
    writer: &Writer,
    path: &Path,
    change: impl FnOnce(&mut Document),
    validate: impl FnOnce() -> Result<(), String>,
) -> Result<bool, WriteError> {
    let (text, fingerprint) = read(path)?;
    let mut doc = Document::parse(&text);
    change(&mut doc);
    let new_text = doc.render();
    if new_text == text {
        return Ok(false);
    }
    writer.write(path, &new_text, fingerprint, validate)?;
    Ok(true)
}

/// `.ssh_config`, `config.d_ceph-cluster.conf`: the parent folder keeps
/// `~/.ssh/config` and `config.d/*` apart.
fn backup_group(path: &Path) -> String {
    let name = |p: Option<&Path>| p.and_then(Path::file_name).map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    format!("{}_{}", name(path.parent()), name(Some(path)))
}

fn replace(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    fs::create_dir_all(dir)?;
    let file_name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let tmp = dir.join(format!(".{file_name}.nativeterm-tmp"));
    let result = (|| {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        platform::swap_in(path, &tmp)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

#[cfg(windows)]
mod platform {
    use std::io;
    use std::path::Path;
    use std::time::Duration;

    use windows::core::HSTRING;
    use windows::Win32::Storage::FileSystem::{
        MoveFileExW, ReplaceFileW, MOVEFILE_WRITE_THROUGH, REPLACEFILE_WRITE_THROUGH,
    };

    /// Existing file: `ReplaceFileW` keeps its ACL and attributes, and is
    /// retried briefly in case ssh has it open. New file: owner-only ACL,
    /// then move into place.
    pub fn swap_in(path: &Path, tmp: &Path) -> io::Result<()> {
        if path.exists() {
            let mut last = None;
            for _ in 0..10 {
                let result = unsafe {
                    ReplaceFileW(&HSTRING::from(path), &HSTRING::from(tmp), None, REPLACEFILE_WRITE_THROUGH, None, None)
                };
                match result {
                    Ok(()) => return Ok(()),
                    Err(e) => last = Some(e),
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(last.expect("at least one attempt").into())
        } else {
            crate::acl::restrict_to_owner(tmp)?;
            unsafe { MoveFileExW(&HSTRING::from(tmp), &HSTRING::from(path), MOVEFILE_WRITE_THROUGH)? };
            Ok(())
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use std::io;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    pub fn swap_in(path: &Path, tmp: &Path) -> io::Result<()> {
        let mode = std::fs::metadata(path).map(|m| m.permissions().mode()).unwrap_or(0o600);
        std::fs::set_permissions(tmp, std::fs::Permissions::from_mode(mode))?;
        std::fs::rename(tmp, path)
    }
}

/// `YYYYMMDD-HHMMSS-mmm` in UTC.
fn timestamp(t: SystemTime) -> String {
    let since = t.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = since.as_secs() as i64;
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}-{:03}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        since.subsec_millis()
    )
}

/// Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok() -> Result<(), String> {
        Ok(())
    }

    fn backups(dir: &Path) -> Vec<PathBuf> {
        let mut all = Vec::new();
        for group in fs::read_dir(dir).into_iter().flatten().flatten() {
            all.extend(fs::read_dir(group.path()).unwrap().flatten().map(|e| e.path()));
        }
        all
    }

    #[test]
    fn timestamps() {
        assert_eq!(timestamp(UNIX_EPOCH), "19700101-000000-000");
        // 2026-09-17 06:05:04.321 UTC
        let t = UNIX_EPOCH + Duration::from_millis(1_789_625_104_321);
        assert_eq!(timestamp(t), "20260917-060504-321");
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }

    #[test]
    fn creates_a_new_file_with_an_owner_only_acl() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.d").join("new.conf");
        let writer = Writer::new(dir.path().join("backups"));
        assert_eq!(writer.write(&path, "Host a\n", None, ok).unwrap(), None);
        assert_eq!(fs::read_to_string(&path).unwrap(), "Host a\n");
        #[cfg(windows)]
        {
            let sddl = crate::acl::dacl_sddl(&path).unwrap();
            assert!(sddl.starts_with("D:P") && sddl.matches("(A;").count() == 3, "{sddl}");
        }
    }

    #[test]
    fn overwrite_keeps_the_acl_and_backs_up() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config");
        fs::write(&path, "old\n").unwrap();
        #[cfg(windows)]
        let custom = {
            let sid = crate::acl::current_user_sid().unwrap();
            crate::acl::set_dacl(&path, &format!("D:P(A;;FA;;;{sid})(A;;FR;;;BU)")).unwrap();
            crate::acl::dacl_sddl(&path).unwrap()
        };
        let writer = Writer::new(dir.path().join("backups"));
        let (_, fp) = read(&path).unwrap();
        let backup = writer.write(&path, "new\n", fp, ok).unwrap().unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "new\n");
        assert_eq!(fs::read_to_string(&backup).unwrap(), "old\n");
        assert!(!dir.path().join(".config.nativeterm-tmp").exists());
        // same entries; ReplaceFileW may add the auto-inherited flag ("D:PAI")
        #[cfg(windows)]
        {
            let entries = |s: &str| s[s.find('(').unwrap()..].to_string();
            let after = crate::acl::dacl_sddl(&path).unwrap();
            assert_eq!(entries(&after), entries(&custom), "{after}");
            assert!(after.starts_with("D:P"), "still protected: {after}");
        }
    }

    #[test]
    fn refuses_when_the_file_changed_meanwhile() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config");
        fs::write(&path, "one\n").unwrap();
        let (_, fp) = read(&path).unwrap();
        fs::write(&path, "two!\n").unwrap();
        let writer = Writer::new(dir.path().join("backups"));
        assert!(matches!(writer.write(&path, "mine\n", fp, ok), Err(WriteError::Conflict)));
        assert_eq!(fs::read_to_string(&path).unwrap(), "two!\n");
        // expecting "absent" while the file exists is a conflict too
        assert!(matches!(writer.write(&path, "mine\n", None, ok), Err(WriteError::Conflict)));
    }

    #[test]
    fn rejected_change_restores_the_previous_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config");
        fs::write(&path, "good\n").unwrap();
        let writer = Writer::new(dir.path().join("backups"));
        let (_, fp) = read(&path).unwrap();
        let err = writer.write(&path, "bad\n", fp, || Err("Bad owner or permissions".into())).unwrap_err();
        match err {
            WriteError::Rejected { reason, backup } => {
                assert_eq!(reason, "Bad owner or permissions");
                assert_eq!(fs::read_to_string(backup.unwrap()).unwrap(), "good\n");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(fs::read_to_string(&path).unwrap(), "good\n");

        let fresh = dir.path().join("fresh.conf");
        assert!(writer.write(&fresh, "bad\n", None, || Err("no".into())).is_err());
        assert!(!fresh.exists(), "a rejected new file is removed");
    }

    #[test]
    fn prunes_old_backups() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config");
        fs::write(&path, "0\n").unwrap();
        let mut writer = Writer::new(dir.path().join("backups"));
        writer.keep = 3;
        for i in 1..=6 {
            let (_, fp) = read(&path).unwrap();
            writer.write(&path, &format!("{i}\n"), fp, ok).unwrap();
        }
        let kept = backups(&writer.backups);
        assert_eq!(kept.len(), 3);
        let mut contents: Vec<String> = kept.iter().map(|p| fs::read_to_string(p).unwrap()).collect();
        contents.sort();
        assert_eq!(contents, vec!["3\n", "4\n", "5\n"], "newest kept");
    }

    #[test]
    fn edit_file_writes_only_real_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("web.conf");
        fs::write(&path, "Host web01\n    HostName 10.0.0.1\n").unwrap();
        let writer = Writer::new(dir.path().join("backups"));
        let changed = edit_file(&writer, &path, |doc| doc.set(1, "NativeTermLabel", "Web 01"), ok).unwrap();
        assert!(changed);
        assert_eq!(fs::read_to_string(&path).unwrap(), "Host web01\n    HostName 10.0.0.1\n    NativeTermLabel \"Web 01\"\n");
        let unchanged = edit_file(&writer, &path, |doc| doc.set(1, "NativeTermLabel", "Web 01"), ok).unwrap();
        assert!(!unchanged);
        assert_eq!(backups(&writer.backups).len(), 1);
    }
}
