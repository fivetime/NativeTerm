//! File and folder dialogs for a helper that has no window of its own
//! (the shim's rz / sz): Windows' own; on Linux `zenity` or `kdialog`,
//! whichever the desktop has (`available` says whether either is
//! there, so the caller can do without a dialog: the Downloads folder,
//! or the files window); on macOS AppleScript's `choose file` /
//! `choose folder` through `osascript`. Cancelled is `None`.

#[cfg(unix)]
use std::path::{Path, PathBuf};

#[cfg(windows)]
pub use native_term_win::picker::{pick_files, pick_folder};

/// Whether a dialog can be shown here at all.
#[cfg(windows)]
#[must_use]
pub fn available() -> bool {
    true
}

/// One path per line, as the dialogs print them.
#[cfg(unix)]
fn paths_in(output: &str) -> Vec<PathBuf> {
    output.lines().map(str::trim).filter(|l| !l.is_empty()).map(PathBuf::from).collect()
}

/// A folder as a dialog's starting place wants it: with a trailing
/// separator, so the dialog opens inside it rather than selecting it.
#[cfg(unix)]
fn inside(start: Option<&Path>) -> Option<String> {
    let start = start.filter(|p| p.is_dir())?;
    let mut text = start.display().to_string();
    if !text.ends_with('/') {
        text.push('/');
    }
    Some(text)
}

#[cfg(all(unix, not(target_os = "macos")))]
mod unix {
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};

    /// The first program on `PATH` of these.
    fn first_on_path(names: &[&str]) -> Option<&'static str> {
        let path = std::env::var_os("PATH")?;
        let dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
        names.iter().find(|n| dirs.iter().any(|d| d.join(n).is_file())).map(|n| match *n {
            "zenity" => "zenity",
            _ => "kdialog",
        })
    }

    fn tool() -> Option<&'static str> {
        first_on_path(&["zenity", "kdialog"])
    }

    #[must_use]
    pub fn available() -> bool {
        tool().is_some()
    }

    /// What the dialog printed when the person chose; `None` on cancel.
    fn run(program: &str, args: &[String]) -> Option<String> {
        let out = Command::new(program).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    }

    pub fn pick_files(title: &str, start: Option<&Path>) -> Option<Vec<PathBuf>> {
        let output = match tool()? {
            "zenity" => {
                let mut args = vec![
                    "--file-selection".into(),
                    "--multiple".into(),
                    "--separator=\n".into(),
                    format!("--title={title}"),
                ];
                if let Some(inside) = super::inside(start) {
                    args.push(format!("--filename={inside}"));
                }
                run("zenity", &args)?
            }
            _ => {
                let start = super::inside(start).unwrap_or_else(|| ".".into());
                let args = vec![
                    "--getopenfilename".into(),
                    start,
                    "--multiple".into(),
                    "--separate-output".into(),
                    "--title".into(),
                    title.to_string(),
                ];
                run("kdialog", &args)?
            }
        };
        let files = super::paths_in(&output);
        (!files.is_empty()).then_some(files)
    }

    pub fn pick_folder(title: &str, start: Option<&Path>) -> Option<PathBuf> {
        let output = match tool()? {
            "zenity" => {
                let mut args = vec!["--file-selection".into(), "--directory".into(), format!("--title={title}")];
                if let Some(inside) = super::inside(start) {
                    args.push(format!("--filename={inside}"));
                }
                run("zenity", &args)?
            }
            _ => {
                let start = super::inside(start).unwrap_or_else(|| ".".into());
                let args = vec!["--getexistingdirectory".into(), start, "--title".into(), title.to_string()];
                run("kdialog", &args)?
            }
        };
        super::paths_in(&output).into_iter().next()
    }
}

#[cfg(target_os = "macos")]
mod unix {
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};

    #[must_use]
    pub fn available() -> bool {
        true
    }

    /// `text` inside an AppleScript string.
    fn quoted(text: &str) -> String {
        format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
    }

    /// Run an AppleScript; what it printed, `None` when the person
    /// cancelled (error -128) or it failed.
    fn osascript(script: &str) -> Option<String> {
        let mut child = Command::new("osascript")
            .arg("-")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        child.stdin.take()?.write_all(script.as_bytes()).ok()?;
        let out = child.wait_with_output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    }

    fn default_location(start: Option<&Path>) -> String {
        super::inside(start).map(|s| format!(" default location (POSIX file {})", quoted(&s))).unwrap_or_default()
    }

    pub fn pick_files(title: &str, start: Option<&Path>) -> Option<Vec<PathBuf>> {
        let script = format!(
            "set chosen to choose file with prompt {} with multiple selections allowed{}\n\
             set out to \"\"\n\
             repeat with f in chosen\n\
             set out to out & POSIX path of f & linefeed\n\
             end repeat\n\
             out",
            quoted(title),
            default_location(start)
        );
        let files = super::paths_in(&osascript(&script)?);
        (!files.is_empty()).then_some(files)
    }

    pub fn pick_folder(title: &str, start: Option<&Path>) -> Option<PathBuf> {
        let script = format!("POSIX path of (choose folder with prompt {}{})", quoted(title), default_location(start));
        super::paths_in(&osascript(&script)?).into_iter().next()
    }
}

#[cfg(unix)]
pub use unix::{available, pick_files, pick_folder};

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn dialog_output_is_one_path_per_line() {
        assert_eq!(paths_in("/a/b.txt\n/a/c d.log\n\n"), [PathBuf::from("/a/b.txt"), PathBuf::from("/a/c d.log")]);
        assert!(paths_in("\n").is_empty());
    }

    #[test]
    fn the_starting_folder_is_entered() {
        let dir = tempfile::tempdir().unwrap();
        let entered = inside(Some(dir.path())).unwrap();
        assert!(entered.ends_with('/'));
        assert_eq!(inside(Some(Path::new("/no/such/folder/here"))), None, "only a folder that exists");
        assert_eq!(inside(None), None);
    }
}
