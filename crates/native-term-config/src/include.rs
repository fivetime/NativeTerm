//! Resolving `Include` patterns the way ssh does for user configs:
//! `~` is the home directory, relative paths are relative to `~/.ssh`,
//! and wildcards expand to matching files in sorted order. Missing files
//! are ignored, as ssh ignores them.

use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Eq)]
pub enum IncludeError {
    /// Wildcards in a directory component: ssh allows them, the tree
    /// doesn't use them (reported, not followed).
    WildcardDirectory(String),
}

pub fn resolve(pattern: &str, home: &Path, ssh_dir: &Path) -> Result<Vec<PathBuf>, IncludeError> {
    let path = if pattern == "~" {
        home.to_path_buf()
    } else if let Some(rest) = pattern.strip_prefix("~/").or_else(|| pattern.strip_prefix("~\\")) {
        home.join(rest)
    } else if Path::new(pattern).is_absolute() {
        PathBuf::from(pattern)
    } else {
        ssh_dir.join(pattern)
    };

    let file_pattern = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let parent = path.parent().unwrap_or(Path::new(""));
    if has_wildcard(&parent.to_string_lossy()) {
        return Err(IncludeError::WildcardDirectory(pattern.to_string()));
    }
    if !has_wildcard(&file_pattern) {
        return Ok(if path.is_file() { vec![path] } else { Vec::new() });
    }
    let Ok(entries) = std::fs::read_dir(parent) else { return Ok(Vec::new()) };
    let mut found: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .filter(|e| glob_match(&file_pattern, &e.file_name().to_string_lossy()))
        .map(|e| e.path())
        .collect();
    found.sort();
    Ok(found)
}

fn has_wildcard(s: &str) -> bool {
    s.contains(['*', '?'])
}

/// `*` and `?` matching, case-insensitive (Windows file names).
pub fn glob_match(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let n: Vec<char> = name.to_lowercase().chars().collect();
    let (mut pi, mut ni) = (0, 0);
    let mut backtrack: Option<(usize, usize)> = None;
    while ni < n.len() {
        match p.get(pi) {
            Some('*') => {
                backtrack = Some((pi, ni));
                pi += 1;
            }
            Some(&c) if c == '?' || c == n[ni] => {
                pi += 1;
                ni += 1;
            }
            _ => match backtrack {
                Some((bp, bn)) => {
                    pi = bp + 1;
                    ni = bn + 1;
                    backtrack = Some((bp, bn + 1));
                }
                None => return false,
            },
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob() {
        assert!(glob_match("*.conf", "ceph-cluster.conf"));
        assert!(glob_match("*.conf", "K8S.CONF"));
        assert!(!glob_match("*.conf", "notes.nt.toml"));
        assert!(glob_match("a?c*", "abcdef"));
        assert!(!glob_match("a?c", "ac"));
        assert!(glob_match("*", ""));
    }

    #[test]
    fn resolves_tilde_relative_and_wildcards() {
        let home = tempfile::tempdir().unwrap();
        let ssh = home.path().join(".ssh");
        let dir = ssh.join("config.d");
        std::fs::create_dir_all(&dir).unwrap();
        for f in ["b.conf", "a.conf", "c.nt.toml"] {
            std::fs::write(dir.join(f), "").unwrap();
        }
        let expected = vec![dir.join("a.conf"), dir.join("b.conf")];
        assert_eq!(resolve("~/.ssh/config.d/*.conf", home.path(), &ssh).unwrap(), expected);
        assert_eq!(resolve("config.d/*.conf", home.path(), &ssh).unwrap(), expected);
        assert_eq!(resolve("config.d/b.conf", home.path(), &ssh).unwrap(), vec![dir.join("b.conf")]);
        assert!(resolve("config.d/missing.conf", home.path(), &ssh).unwrap().is_empty());
        assert!(matches!(resolve("*/x.conf", home.path(), &ssh), Err(IncludeError::WildcardDirectory(_))));
    }
}
