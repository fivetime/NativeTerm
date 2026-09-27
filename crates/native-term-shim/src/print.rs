//! The print preview of text a terminal hands over in a file (its
//! selection), where the system has no print panel for the terminal's
//! window (everywhere but macOS): the text as a page in the person's
//! browser, which opens its print preview for it. Nothing is printed
//! unless the person says so there; the browser knows the printers, the
//! paper and the fonts, those of every script included.
//!
//! The file is one the terminal wrote in a folder of the person's own
//! (the session's runtime folder, the temporary folder) and is taken away
//! at once; the page is there for a minute, for the browser to read.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// What the terminal names its files, and the shim its pages.
const PREFIX: &str = "nativeterm-print-";
/// How long the browser has to read the page.
const SHOWN: Duration = Duration::from_secs(60);
/// A page older than this was left by a shim that was ended early.
const STALE: Duration = Duration::from_secs(3600);

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The page: the text as it was on the terminal, in a fixed pitch font,
/// black on white, long lines folded; the print preview asked for when it
/// has loaded.
pub fn page(text: &str, title: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<title>{title}</title>
<style>
@page {{ margin: 15mm; }}
html, body {{ margin: 0; background: #fff; color: #000; }}
pre {{
  margin: 0;
  font: 10pt/1.35 ui-monospace, "Cascadia Mono", Consolas, Menlo, "DejaVu Sans Mono", monospace;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}}
@media screen {{ body {{ padding: 15mm; }} }}
</style>
</head>
<body>
<pre>{text}</pre>
<script>addEventListener("load", function () {{ print(); }});</script>
</body>
</html>
"#,
        title = escape(title),
        text = escape(text),
    )
}

/// Whether `file` is one of the terminal's: named as they are, where it
/// is said to be (the shim reads and removes nothing else).
fn ours(file: &Path) -> bool {
    file.is_absolute()
        && file.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(PREFIX) && n.ends_with(".txt"))
}

/// Pages and files a shim that was ended early left in `dir`.
fn sweep(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|at| now.duration_since(at).is_ok_and(|age| age > STALE));
        if old && entry.file_name().to_str().is_some_and(|n| n.starts_with(PREFIX)) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// A new file for the page, the person's alone.
fn create(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn show(file: &Path, title: &str) -> std::io::Result<PathBuf> {
    let text = std::fs::read(file)?;
    let _ = std::fs::remove_file(file);
    let text = String::from_utf8_lossy(&text);
    let page_path = file.with_extension("html");
    create(&page_path)?.write_all(page(&text, title).as_bytes())?;
    if let Err(e) = native_term_os::shell::open_file(&page_path) {
        let _ = std::fs::remove_file(&page_path);
        return Err(e);
    }
    Ok(page_path)
}

/// Exit code 0 when the page was handed to the browser, 1 otherwise. Stays
/// while the browser reads the page, then takes it away.
pub fn preview(file: &str, title: Option<&str>) -> i32 {
    let file = Path::new(file);
    if !ours(file) {
        eprintln!("--print-preview: not a file of the terminal's: {}", file.display());
        return 1;
    }
    if let Some(dir) = file.parent() {
        sweep(dir);
    }
    match show(file, title.filter(|t| !t.trim().is_empty()).unwrap_or("NativeTerm")) {
        Ok(page) => {
            std::thread::sleep(SHOWN);
            let _ = std::fs::remove_file(page);
            0
        }
        Err(e) => {
            eprintln!("--print-preview: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_text_is_shown_as_it_is() {
        let page = page("a <b> & \"c\"\n  ls -l > out\n中文", "web01 <1>");
        assert!(page.contains("<pre>a &lt;b&gt; &amp; \"c\"\n  ls -l &gt; out\n中文</pre>"));
        assert!(page.contains("<title>web01 &lt;1&gt;</title>"));
        assert!(page.contains("<meta charset=\"utf-8\">"));
        assert!(page.contains("print();"), "the preview is asked for");
        // a script in the text is text
        let evil = super::page("</pre><script>alert(1)</script>", "x");
        assert!(!evil.contains("<script>alert"));
    }

    #[test]
    fn only_the_terminals_files() {
        let dir = std::env::temp_dir();
        assert!(ours(&dir.join("nativeterm-print-1700000000-42.txt")));
        assert!(!ours(&dir.join("notes.txt")));
        assert!(!ours(&dir.join("nativeterm-print-1.html")));
        assert!(!ours(Path::new("nativeterm-print-1.txt")), "said where it is");
    }

    #[test]
    fn the_file_becomes_the_page() {
        let dir = std::env::temp_dir().join(format!("nativeterm-print-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let page_path = dir.join("nativeterm-print-1-2.html");
        let mut file = create(&page_path).unwrap();
        file.write_all(page("x", "y").as_bytes()).unwrap();
        assert!(create(&page_path).is_err(), "never over a file that is there");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&page_path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        // young: left alone
        sweep(&dir);
        assert!(page_path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
