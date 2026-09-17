//! `Host` aliases: one namespace across all files, no spaces, and `*`, `?`,
//! `!` are pattern characters. Labels (display names) are free text, so
//! aliases are generated from them.

use std::collections::HashSet;

/// A `Host` pattern that names one host (no wildcard, not negated).
pub fn is_literal(pattern: &str) -> bool {
    !pattern.is_empty() && !pattern.contains(['*', '?', '!'])
}

/// Lowercase ASCII letters, digits, `-`, `_`, `.`; Han characters become
/// pinyin without tones (`控制节点` → `kongzhijiedian`); anything else
/// becomes a single `-`. Never starts with `-` (ssh would read it as an
/// option).
pub fn sanitize(label: &str) -> String {
    use pinyin::ToPinyin;
    let mut out = String::new();
    for c in label.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
            out.push(c);
        } else if let Some(p) = c.to_pinyin() {
            out.push_str(p.plain());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches(|c| c == '-' || c == '.').to_string();
    if out.is_empty() {
        "host".to_string()
    } else {
        out
    }
}

/// A globally unique alias for `label`. `taken` holds existing aliases in
/// lowercase (ssh matches host names case-insensitively). On a clash the
/// folder is prefixed, then a number is appended.
pub fn unique(label: &str, folder: &str, taken: &HashSet<String>) -> String {
    let base = sanitize(label);
    if !taken.contains(&base) {
        return base;
    }
    let prefixed = format!("{}.{base}", sanitize(folder));
    if !taken.contains(&prefixed) {
        return prefixed;
    }
    (2..)
        .map(|n| format!("{prefixed}-{n}"))
        .find(|candidate| !taken.contains(candidate))
        .expect("unbounded range")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_patterns() {
        assert!(is_literal("node01"));
        assert!(is_literal("10.32.32.130"));
        assert!(!is_literal("incus-node-*"));
        assert!(!is_literal("!bastion"));
        assert!(!is_literal("web?"));
    }

    #[test]
    fn sanitizes_labels() {
        assert_eq!(sanitize("10.32.16.66(osp-control1)"), "10.32.16.66-osp-control1");
        assert_eq!(sanitize("Web 01 / Prod"), "web-01-prod");
        assert_eq!(sanitize("控制节点"), "kongzhijiedian");
        assert_eq!(sanitize("Ceph 集群"), "ceph-jiqun");
        assert_eq!(sanitize("控制节点0"), "kongzhijiedian0");
        assert_eq!(sanitize("✓✓"), "host");
        assert_eq!(sanitize("--x--"), "x");
        assert_eq!(sanitize("K8s_Master.local"), "k8s_master.local");
    }

    #[test]
    fn unique_prefixes_then_numbers() {
        let mut taken: HashSet<String> = ["web01".to_string()].into();
        assert_eq!(unique("web01", "prod", &taken), "prod.web01");
        taken.insert("prod.web01".into());
        assert_eq!(unique("WEB01", "Prod", &taken), "prod.web01-2");
        assert_eq!(unique("db01", "prod", &taken), "db01");
    }
}
