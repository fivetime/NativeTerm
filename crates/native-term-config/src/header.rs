//! The two lines NativeTerm needs at the top of `~/.ssh/config`:
//!
//! ```text
//! IgnoreUnknown NativeTerm*
//! Include ~/.ssh/config.d/*.conf
//! ```
//!
//! Both must be in the global part (an `Include` inside a `Host` block is
//! conditional). `IgnoreUnknown` must come before the first `NativeTerm*`
//! key in parse order, i.e. before the `Include`; and ssh keeps only the
//! first `IgnoreUnknown` it sees, so an existing one is extended instead of
//! adding a second.

use crate::document::{BlockKind, Document};

pub const IGNORE_PATTERN: &str = "NativeTerm*";
pub const DEFAULT_INCLUDE: &str = "~/.ssh/config.d/*.conf";

/// Make sure the header is present; returns whether the document changed.
pub fn ensure(doc: &mut Document, include: &str) -> bool {
    let mut changed = false;
    let global = 0;
    debug_assert_eq!(doc.blocks()[global].kind, BlockKind::Global);

    // IgnoreUnknown: extend the first one, or add one at the top.
    let ignore_line = match first_line(doc, "IgnoreUnknown") {
        Some(line) => {
            let d = doc.lines[line].directive().expect("directive");
            let mut patterns: Vec<String> = d.value().split(',').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect();
            if !patterns.iter().any(|p| p.eq_ignore_ascii_case(IGNORE_PATTERN)) {
                patterns.push(IGNORE_PATTERN.to_string());
                doc.set(global, "IgnoreUnknown", &patterns.join(","));
                changed = true;
            }
            line
        }
        None => {
            changed = true;
            doc.insert_global_first("IgnoreUnknown", IGNORE_PATTERN)
        }
    };

    // It must precede every Include; move it up if needed.
    let mut ignore_line = ignore_line;
    if let Some(first_include) = first_line(doc, "Include") {
        if first_include < ignore_line {
            let line = doc.lines.remove(ignore_line);
            doc.lines.insert(first_include, line);
            ignore_line = first_include;
            changed = true;
        }
    }

    let already = global_directive_lines(doc, "Include")
        .into_iter()
        .any(|i| doc.lines[i].directive().is_some_and(|d| d.args.iter().any(|a| same_path(a, include))));
    if !already {
        doc.insert_line(ignore_line + 1, "Include", include);
        changed = true;
    }
    changed
}

/// Point NativeTerm's `Include` (the one naming `old`) at `new` instead,
/// keeping the line's other patterns; add it if there was none. Returns
/// whether the document changed.
pub fn replace_include(doc: &mut Document, old: &str, new: &str) -> bool {
    let line = global_directive_lines(doc, "Include")
        .into_iter()
        .find(|&i| doc.lines[i].directive().is_some_and(|d| d.args.iter().any(|a| same_path(a, old))));
    let changed = match line {
        Some(i) => {
            let d = doc.lines[i].directive().expect("directive");
            let args: Vec<String> = d
                .args
                .iter()
                .map(|a| if same_path(a, old) { new.to_string() } else { a.clone() })
                .map(|a| crate::document::quote_arg(&a))
                .collect();
            let indent: String = doc.lines[i].text.chars().take_while(|c| c.is_whitespace()).collect();
            let keyword = d.keyword.clone();
            let text = format!("{indent}{keyword} {}", args.join(" "));
            let changed = text != doc.lines[i].text;
            doc.lines[i] = crate::document::parse_line(&text);
            changed
        }
        None => false,
    };
    ensure(doc, new) || changed
}

fn global_directive_lines(doc: &Document, keyword: &str) -> Vec<usize> {
    let global = doc.blocks().swap_remove(0);
    doc.directives(&global).filter(|(_, d)| d.is(keyword)).map(|(i, _)| i).collect()
}

fn first_line(doc: &Document, keyword: &str) -> Option<usize> {
    global_directive_lines(doc, keyword).into_iter().next()
}

fn same_path(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.replace('\\', "/").to_ascii_lowercase();
    norm(a) == norm(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn include_moves_elsewhere() {
        let mut doc = Document::parse("IgnoreUnknown NativeTerm*\nInclude ~/.ssh/config.d/*.conf other/*.conf\n\nHost a\n    HostName x\n");
        assert!(replace_include(&mut doc, DEFAULT_INCLUDE, "D:/Sync/ssh folders/*.conf"));
        assert_eq!(
            doc.render(),
            "IgnoreUnknown NativeTerm*\nInclude \"D:/Sync/ssh folders/*.conf\" other/*.conf\n\nHost a\n    HostName x\n"
        );
        assert!(!replace_include(&mut doc, "D:/Sync/ssh folders/*.conf", "D:/Sync/ssh folders/*.conf"));
        // no such line yet: added like the header would be
        let mut doc = Document::parse("Host a\n    HostName x\n");
        assert!(replace_include(&mut doc, DEFAULT_INCLUDE, "D:/x/*.conf"));
        assert!(doc.render().starts_with("IgnoreUnknown NativeTerm*\nInclude D:/x/*.conf\n"), "{}", doc.render());
    }

    #[test]
    fn adds_both_lines_after_leading_comments() {
        let mut doc = Document::parse("# mine\nHost a\n  User x\n");
        assert!(ensure(&mut doc, DEFAULT_INCLUDE));
        assert_eq!(
            doc.render(),
            "# mine\nIgnoreUnknown NativeTerm*\nInclude ~/.ssh/config.d/*.conf\nHost a\n  User x\n"
        );
        assert!(!ensure(&mut doc, DEFAULT_INCLUDE), "idempotent");
    }

    #[test]
    fn extends_existing_ignore_unknown_instead_of_adding_a_second() {
        let mut doc = Document::parse("IgnoreUnknown UseKeychain\nInclude ~/.ssh/config.d/*.conf\n");
        assert!(ensure(&mut doc, DEFAULT_INCLUDE));
        assert_eq!(doc.render(), "IgnoreUnknown UseKeychain,NativeTerm*\nInclude ~/.ssh/config.d/*.conf\n");
    }

    #[test]
    fn moves_ignore_unknown_before_the_include() {
        let mut doc = Document::parse("Include ~/.ssh/config.d/*.conf\nIgnoreUnknown NativeTerm*\n");
        assert!(ensure(&mut doc, DEFAULT_INCLUDE));
        assert_eq!(doc.render(), "IgnoreUnknown NativeTerm*\nInclude ~/.ssh/config.d/*.conf\n");
    }

    #[test]
    fn include_inside_a_host_block_does_not_count() {
        let mut doc = Document::parse("Host *\n  Include ~/.ssh/config.d/*.conf\n");
        assert!(ensure(&mut doc, "~/.ssh/config.d/*.conf"));
        let text = doc.render();
        assert!(text.starts_with("IgnoreUnknown NativeTerm*\nInclude ~/.ssh/config.d/*.conf\nHost *\n"), "{text}");
    }

    #[test]
    fn empty_file() {
        let mut doc = Document::parse("");
        assert!(ensure(&mut doc, DEFAULT_INCLUDE));
        doc.trailing_newline = true;
        assert_eq!(doc.render(), "IgnoreUnknown NativeTerm*\nInclude ~/.ssh/config.d/*.conf\n");
    }
}
