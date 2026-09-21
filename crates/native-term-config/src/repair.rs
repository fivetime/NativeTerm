//! Putting NativeTerm's own path right in the files it wrote, after the
//! program folder moved (renamed, copied to another drive, a portable
//! folder carried to another computer).
//!
//! The only thing in an ssh config that names NativeTerm is the
//! `ProxyCommand` of a host with a proxy: `"<…>\nativeterm-shim.exe"
//! --proxy <url> %h %p` (see `proxy.rs`). ssh runs it with the shell's
//! working directory, so the path has to be absolute; when the folder
//! moves, the line points at nothing.
//!
//! A line is rewritten only when the program it names **is not there on
//! this computer**. A config shared between computers (synced) then keeps
//! working on both: each one repairs what is broken for it and leaves the
//! rest alone. Anything that is not a `ProxyCommand` NativeTerm wrote is
//! never touched.

use std::path::{Path, PathBuf};

use crate::document::{Document, LineKind};
use crate::proxy::Proxy;
use crate::write::{edit_file, WriteError, Writer};

/// What a repair pass did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Repaired {
    /// The files that were changed.
    pub files: Vec<PathBuf>,
    /// How many lines in all.
    pub lines: usize,
    /// The paths that were pointed at and are not there.
    pub was: Vec<PathBuf>,
}

impl Repaired {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// The program a `ProxyCommand` NativeTerm wrote names, or `None` for any
/// other command.
fn shim_of(value: &str) -> Option<PathBuf> {
    Proxy::from_command(value)?;
    let value = value.trim();
    let program = match value.strip_prefix('"') {
        Some(quoted) => quoted.split_once('"')?.0,
        None => value.split_once(' ')?.0,
    };
    Some(PathBuf::from(program))
}

/// The lines of `doc` to put right, with what they name (the caller says
/// which of those are gone).
fn ours(doc: &Document) -> Vec<(usize, PathBuf, Proxy)> {
    doc.lines
        .iter()
        .enumerate()
        .filter_map(|(at, line)| {
            let directive = line.directive()?;
            if !directive.is("ProxyCommand") {
                return None;
            }
            let value = line.raw_value()?;
            let proxy = Proxy::from_command(value)?;
            Some((at, shim_of(value)?, proxy))
        })
        .collect()
}

/// Rewrites, in `path`, the `ProxyCommand` lines that name a helper which
/// isn't there, so that they name `shim`. Whether anything changed, and
/// what was named before.
fn repair_file(
    writer: &Writer,
    path: &Path,
    shim: &Path,
    exists: &dyn Fn(&Path) -> bool,
) -> Result<(usize, Vec<PathBuf>), WriteError> {
    let mut was = Vec::new();
    let mut count = 0;
    let changed = edit_file(
        writer,
        path,
        |doc| {
            for (at, named, proxy) in ours(doc) {
                if named == shim || exists(&named) {
                    continue; // ours already, or a helper that is there
                }
                let line = &mut doc.lines[at];
                let indent = line.text[..line.text.len() - line.text.trim_start().len()].to_string();
                let keyword = match &line.kind {
                    LineKind::Directive(d) => d.keyword.clone(),
                    _ => continue,
                };
                line.text = format!("{indent}{keyword} {}", proxy.command(shim));
                line.kind = Document::parse(&line.text).lines.remove(0).kind;
                was.push(named);
                count += 1;
            }
        },
        || Ok(()),
    )?;
    if !changed {
        return Ok((0, Vec::new()));
    }
    Ok((count, was))
}

/// Puts NativeTerm's path right in `files` (the main config and the folder
/// files). Files that can't be read or written are skipped: a repair is
/// never worth failing a start.
pub fn proxy_commands(writer: &Writer, files: &[PathBuf], shim: &Path) -> Repaired {
    proxy_commands_with(writer, files, shim, &|path: &Path| path.exists())
}

/// As `proxy_commands`, with the "is it there" test given (for tests).
pub fn proxy_commands_with(
    writer: &Writer,
    files: &[PathBuf],
    shim: &Path,
    exists: &dyn Fn(&Path) -> bool,
) -> Repaired {
    let mut done = Repaired::default();
    for file in files {
        match repair_file(writer, file, shim, exists) {
            Ok((0, _)) => {}
            Ok((lines, was)) => {
                done.files.push(file.clone());
                done.lines += lines;
                for path in was {
                    if !done.was.contains(&path) {
                        done.was.push(path);
                    }
                }
            }
            Err(_) => {}
        }
    }
    done
}

#[cfg(test)]
mod tests {
    use super::*;

    fn writer(dir: &Path) -> Writer {
        Writer::new(dir.join("backups"))
    }

    #[test]
    fn a_helper_that_is_gone_is_pointed_at_this_program() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        std::fs::write(
            &config,
            concat!(
                "Host proxied\n",
                "    HostName db.lan\n",
                "    ProxyCommand \"D:\\Old\\nativeterm-shim.exe\" --proxy socks5://gw:1080 %h %p\n",
                "\n",
                "Host own-command\n",
                "    ProxyCommand /usr/bin/corkscrew gw 8080 %h %p\n",
            ),
        )
        .unwrap();
        let shim = Path::new(r"C:\New\nativeterm-shim.exe");
        let done = proxy_commands_with(&writer(dir.path()), std::slice::from_ref(&config), shim, &|_| false);
        assert_eq!(done.lines, 1);
        assert_eq!(done.was, vec![PathBuf::from(r"D:\Old\nativeterm-shim.exe")]);
        let text = std::fs::read_to_string(&config).unwrap();
        assert!(
            text.contains(r#"    ProxyCommand "C:\New\nativeterm-shim.exe" --proxy socks5://gw:1080 %h %p"#),
            "{text}"
        );
        // the user's own command, the indent and the rest are untouched
        assert!(text.contains("    ProxyCommand /usr/bin/corkscrew gw 8080 %h %p"), "{text}");
        assert!(text.contains("    HostName db.lan"), "{text}");
    }

    #[test]
    fn a_helper_that_is_there_is_left_alone() {
        // another computer's path in a synced config, or a second copy of
        // NativeTerm: not ours to change
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        let line = "    ProxyCommand \"D:\\Other\\nativeterm-shim.exe\" --proxy http://gw:8080 %h %p\n";
        std::fs::write(&config, format!("Host a\n{line}")).unwrap();
        let done = proxy_commands_with(
            &writer(dir.path()),
            std::slice::from_ref(&config),
            Path::new(r"C:\New\nativeterm-shim.exe"),
            &|_| true,
        );
        assert!(done.is_empty());
        assert!(std::fs::read_to_string(&config).unwrap().contains(r"D:\Other\nativeterm-shim.exe"));
    }

    #[test]
    fn a_line_that_already_names_this_program_is_not_rewritten() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        let shim = Path::new(r"C:\New\nativeterm-shim.exe");
        let text = format!("Host a\n    ProxyCommand \"{}\" --proxy socks5://gw:1080 %h %p\n", shim.display());
        std::fs::write(&config, &text).unwrap();
        let done = proxy_commands_with(&writer(dir.path()), std::slice::from_ref(&config), shim, &|_| false);
        assert!(done.is_empty());
        assert_eq!(std::fs::read_to_string(&config).unwrap(), text);
    }

    #[test]
    fn the_proxy_and_its_login_survive_the_repair() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        std::fs::write(
            &config,
            "Host a\n    ProxyCommand \"D:\\Old\\nativeterm-shim.exe\" --proxy socks5://ops@gw.example:1080 %h %p\n",
        )
        .unwrap();
        let shim = Path::new(r"C:\New\nativeterm-shim.exe");
        proxy_commands_with(&writer(dir.path()), std::slice::from_ref(&config), shim, &|_| false);
        let text = std::fs::read_to_string(&config).unwrap();
        let value = text.lines().find_map(|l| l.trim().strip_prefix("ProxyCommand ")).unwrap();
        let proxy = Proxy::from_command(value).expect("still ours");
        assert_eq!(proxy.url(), "socks5://ops@gw.example:1080");
        assert!(value.starts_with(&format!("\"{}\"", shim.display())), "{value}");
    }

    #[test]
    fn a_file_that_is_not_there_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let done = proxy_commands_with(
            &writer(dir.path()),
            &[dir.path().join("gone.conf")],
            Path::new(r"C:\New\nativeterm-shim.exe"),
            &|_| false,
        );
        assert!(done.is_empty());
    }
}
