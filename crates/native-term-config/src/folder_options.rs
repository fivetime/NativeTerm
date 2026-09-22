//! Session options for every host in a folder file. ssh has no notion of
//! folders, so each host in the file carries `Tag nativeterm-<file>` and a
//! `Match tagged nativeterm-<file>` block at the end of the file holds the
//! options. ssh reads the file in order, so the block must come after the
//! hosts (their `Tag` is set by then), and a host's own values still win
//! (the first value ssh sees is kept). Needs OpenSSH 9.4 or newer: older
//! versions reject `Tag` and `Match tagged`.

use std::path::Path;

use crate::alias;
use crate::document::{parse_line, BlockKind, Document, LineKind};
use crate::options::{self, Values};
use crate::tree::FOLDER_DEFAULTS_HOST;

const TAG_PREFIX: &str = "nativeterm-";

/// The tag of a folder file's hosts.
pub fn tag_for(file: &Path) -> String {
    let stem = file.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    format!("{TAG_PREFIX}{}", alias::sanitize(&stem))
}

/// Whether `ssh -V` output names a version with `Tag` (9.4+).
pub fn supported(version: &str) -> bool {
    let Some(at) = version.find("OpenSSH_") else { return false };
    let rest = &version[at + "OpenSSH_".len()..];
    let rest = rest.strip_prefix("for_Windows_").unwrap_or(rest);
    let mut numbers = rest.split(|c: char| !c.is_ascii_digit()).filter(|s| !s.is_empty());
    let major: u32 = numbers.next().and_then(|n| n.parse().ok()).unwrap_or(0);
    let minor: u32 = numbers.next().and_then(|n| n.parse().ok()).unwrap_or(0);
    (major, minor) >= (9, 4)
}

fn is_our_block(kind: &BlockKind, tag: &str) -> bool {
    match kind {
        BlockKind::Match(criteria) => {
            let words: Vec<&str> = criteria.split_whitespace().collect();
            words.len() == 2 && words[0].eq_ignore_ascii_case("tagged") && words[1] == tag
        }
        _ => false,
    }
}

fn find_block(doc: &Document, tag: &str) -> Option<usize> {
    doc.blocks().iter().position(|b| is_our_block(&b.kind, tag))
}

/// Blocks of connectable hosts (not the folder's own defaults block).
fn host_blocks(doc: &Document) -> Vec<usize> {
    doc.blocks()
        .iter()
        .enumerate()
        .filter(|(_, b)| match &b.kind {
            BlockKind::Host(patterns) => {
                patterns.iter().any(|p| alias::is_literal(p))
                    && !patterns.iter().any(|p| p.eq_ignore_ascii_case(FOLDER_DEFAULTS_HOST))
            }
            _ => false,
        })
        .map(|(i, _)| i)
        .collect()
}

fn first_alias(doc: &Document, block: usize) -> String {
    match &doc.blocks()[block].kind {
        BlockKind::Host(patterns) => patterns.iter().find(|p| alias::is_literal(p)).cloned().unwrap_or_default(),
        _ => String::new(),
    }
}

/// The folder's options (empty when it has none).
pub fn read(doc: &Document, tag: &str) -> Values {
    match find_block(doc, tag) {
        Some(block) => options::read(doc, block),
        None => options::empty(),
    }
}

pub fn has_options(doc: &Document, tag: &str) -> bool {
    find_block(doc, tag).is_some()
}

/// Remove NativeTerm's folder tags (any folder) from one host block.
pub fn untag(doc: &mut Document, block: usize) {
    let b = doc.blocks().swap_remove(block);
    let doomed: Vec<usize> =
        doc.directives(&b).filter(|(_, d)| d.is("Tag") && d.value().starts_with(TAG_PREFIX)).map(|(i, _)| i).collect();
    for i in doomed.into_iter().rev() {
        doc.lines.remove(i);
    }
}

/// Keep the folder's options working after hosts were added: every host
/// gets the tag, and the options block goes to the end. Returns the
/// aliases that keep a tag of their own (they don't get the options).
pub fn arrange(doc: &mut Document, tag: &str) -> Vec<String> {
    let Some(block) = find_block(doc, tag) else { return Vec::new() };
    // move the block to the end, unless it is there already
    if block + 1 != doc.blocks().len() {
        let b = doc.blocks().swap_remove(block);
        let mut end = b.end;
        while end > b.start && doc.lines[end - 1].kind == LineKind::Blank {
            end -= 1;
        }
        let lines: Vec<_> = doc.lines.drain(b.start..end).collect();
        if doc.lines.last().is_some_and(|l| l.kind != LineKind::Blank) {
            doc.lines.push(parse_line(""));
        }
        doc.lines.extend(lines);
        doc.trailing_newline = true;
    }
    let mut own_tags = Vec::new();
    for host in host_blocks(doc) {
        let current = doc.get(host, "Tag").map(|d| d.value());
        match current {
            Some(t) if t == tag => {}
            Some(t) if !t.starts_with(TAG_PREFIX) => own_tags.push(first_alias(doc, host)),
            _ => doc.set(host, "Tag", tag),
        }
    }
    own_tags
}

/// Set the folder's options. With none left, the block and the tags go.
pub fn apply(doc: &mut Document, tag: &str, values: &Values) -> Vec<String> {
    let empty = values.values().all(Vec::is_empty);
    match (find_block(doc, tag), empty) {
        (None, true) => Vec::new(),
        (Some(block), true) => {
            doc.remove_block(block);
            for host in host_blocks(doc) {
                if doc.get(host, "Tag").is_some_and(|d| d.value() == tag) {
                    doc.remove(host, "Tag");
                }
            }
            Vec::new()
        }
        (existing, false) => {
            if existing.is_none() {
                if doc.lines.last().is_some_and(|l| l.kind != LineKind::Blank) {
                    doc.lines.push(parse_line(""));
                }
                doc.lines.push(parse_line(&format!("Match tagged {tag}")));
                doc.trailing_newline = true;
            }
            let block = find_block(doc, tag).expect("just made");
            options::apply(doc, block, values);
            arrange(doc, tag)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "\
Host __nativeterm_folder__
    NativeTermLabel 生产

Host web
    HostName 10.0.0.1
    Compression no

Host own
    HostName 10.0.0.2
    Tag mine
";

    fn values(pairs: &[(&'static str, &str)]) -> Values {
        let mut v = options::empty();
        for (k, value) in pairs {
            v.insert(k, vec![value.to_string()]);
        }
        options::normalize(&v).unwrap()
    }

    #[test]
    fn versions() {
        assert!(supported("OpenSSH_for_Windows_9.5p2, LibreSSL 3.8.2"));
        assert!(supported("OpenSSH_9.4p1 Ubuntu"));
        assert!(supported("OpenSSH_10.0p2"));
        assert!(!supported("OpenSSH_for_Windows_8.1p1, LibreSSL 3.0.2"));
        assert!(!supported("OpenSSH_9.3p1"));
        assert!(!supported("PuTTY"));
        let file = Path::new("x").join("config.d").join("sheng-chan.conf");
        assert_eq!(tag_for(&file), "nativeterm-sheng-chan");
    }

    #[test]
    fn options_tags_and_order() {
        let mut doc = Document::parse(FILE);
        let tag = "nativeterm-prod";
        let own = apply(&mut doc, tag, &values(&[("ProxyJump", "gw"), ("Compression", "yes")]));
        assert_eq!(own, ["own"]);
        assert_eq!(
            doc.render(),
            "\
Host __nativeterm_folder__
    NativeTermLabel 生产

Host web
    HostName 10.0.0.1
    Compression no
    Tag nativeterm-prod

Host own
    HostName 10.0.0.2
    Tag mine

Match tagged nativeterm-prod
    Compression yes
    ProxyJump gw
"
        );
        assert_eq!(read(&doc, tag)["ProxyJump"], ["gw"]);

        // a host added after the block: tagged, block moved behind it
        doc.append_host(&["db"], &[("HostName", "10.0.0.3")]);
        assert_eq!(arrange(&mut doc, tag), ["own"]);
        let text = doc.render();
        assert!(text.ends_with("Host db\n    HostName 10.0.0.3\n    Tag nativeterm-prod\n\nMatch tagged nativeterm-prod\n    Compression yes\n    ProxyJump gw\n"), "{text}");
        assert!(
            !text.contains("__nativeterm_folder__\n    NativeTermLabel 生产\n    Tag"),
            "the defaults block isn't a host"
        );

        // a host moved in from another folder keeps no foreign tag
        let db = doc.find_host_block("db").unwrap();
        doc.set(db, "Tag", "nativeterm-other");
        arrange(&mut doc, tag);
        let db = doc.find_host_block("db").unwrap();
        assert_eq!(doc.get(db, "Tag").unwrap().value(), tag);
        assert_eq!(doc.get_all(db, "Tag").len(), 1);
        untag(&mut doc, db);
        let db = doc.find_host_block("db").unwrap();
        assert!(doc.get(db, "Tag").is_none());
        arrange(&mut doc, tag);

        // no options left: back to the start
        apply(&mut doc, tag, &options::empty());
        let text = doc.render();
        assert!(!text.contains("Match") && !text.contains("nativeterm-prod"), "{text}");
        assert!(text.contains("Tag mine"));
    }
}
