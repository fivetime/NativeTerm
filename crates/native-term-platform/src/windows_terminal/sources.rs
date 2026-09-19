//! Turning a profile source (Terminal's own SSH profiles) off or on in
//! `settings.json`: an explicit user action, backed up first. The file is
//! JSON with comments, so the edit is textual: only the entry inside the
//! top-level `disabledProfileSources` array is added or removed, and
//! everything else (comments, order, spacing) stays as it is.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super::jsonc;

/// Terminal's generator for hosts in `~/.ssh/config` (1.25+).
pub const SSH_SOURCE: &str = "Windows.Terminal.SSH";

const KEY: &str = "disabledProfileSources";

/// Whether `source` is listed in the settings text.
pub fn is_disabled(text: &str, source: &str) -> bool {
    jsonc::parse(text)
        .ok()
        .and_then(|v| v.get(KEY).and_then(|a| a.as_array()).cloned())
        .is_some_and(|list| list.iter().any(|s| s.as_str() == Some(source)))
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Str,
    Punct(char),
    Other,
}

#[derive(Clone, Copy, Debug)]
struct Token {
    kind: Kind,
    start: usize,
    end: usize,
}

/// Tokens outside comments and whitespace.
fn tokens(text: &str) -> Vec<Token> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b' ' | b'\t' | b'\r' | b'\n' => i += 1,
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                i = text[i..].find('\n').map_or(bytes.len(), |n| i + n);
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i = text[i + 2..].find("*/").map_or(bytes.len(), |n| i + 2 + n + 2);
            }
            b'"' => {
                let start = i;
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
                i = (i + 1).min(bytes.len());
                out.push(Token { kind: Kind::Str, start, end: i });
            }
            b'{' | b'}' | b'[' | b']' | b',' | b':' => {
                out.push(Token { kind: Kind::Punct(c as char), start: i, end: i + 1 });
                i += 1;
            }
            _ => {
                let start = i;
                while i < bytes.len() && !b" \t\r\n{}[],:\"/".contains(&bytes[i]) {
                    i += 1;
                }
                i = i.max(start + 1);
                out.push(Token { kind: Kind::Other, start, end: i });
            }
        }
    }
    out
}

fn string_is(text: &str, t: &Token, value: &str) -> bool {
    t.kind == Kind::Str && &text[t.start + 1..t.end.saturating_sub(1).max(t.start + 1)] == value
}

/// The indentation of the first top-level member, for inserted lines.
fn member_indent(text: &str, toks: &[Token]) -> String {
    toks.get(1)
        .filter(|t| t.kind == Kind::Str)
        .map(|t| {
            let line_start = text[..t.start].rfind('\n').map_or(0, |n| n + 1);
            let indent = &text[line_start..t.start];
            if indent.trim().is_empty() {
                indent.to_string()
            } else {
                "    ".to_string()
            }
        })
        .unwrap_or_else(|| "    ".to_string())
}

/// `text` with `source` added to (or removed from) the list. `Ok(None)`:
/// nothing to change.
pub fn edit(text: &str, source: &str, disable: bool) -> Result<Option<String>, String> {
    let root = jsonc::parse(text).map_err(|e| format!("settings.json: {e}"))?;
    if !root.is_object() {
        return Err("settings.json: not an object".into());
    }
    if is_disabled(text, source) == disable {
        return Ok(None);
    }
    let toks = tokens(text);
    if toks.first().map(|t| t.kind) != Some(Kind::Punct('{')) {
        return Err("settings.json: unexpected start".into());
    }
    // the top-level member named KEY, and its array's tokens
    let mut depth = 0;
    let mut array: Option<(usize, usize)> = None;
    let mut i = 0;
    while i < toks.len() {
        match toks[i].kind {
            Kind::Punct('{') | Kind::Punct('[') => depth += 1,
            Kind::Punct('}') | Kind::Punct(']') => depth -= 1,
            Kind::Str
                if depth == 1
                    && string_is(text, &toks[i], KEY)
                    && toks.get(i + 1).map(|t| t.kind) == Some(Kind::Punct(':')) =>
            {
                let open = i + 2;
                if toks.get(open).map(|t| t.kind) != Some(Kind::Punct('[')) {
                    return Err(format!("settings.json: {KEY} isn't a list"));
                }
                let mut d = 0;
                let close = (open..toks.len()).find(|&j| {
                    match toks[j].kind {
                        Kind::Punct('[') | Kind::Punct('{') => d += 1,
                        Kind::Punct(']') | Kind::Punct('}') => d -= 1,
                        _ => {}
                    }
                    d == 0
                });
                array = Some((open, close.ok_or("settings.json: unclosed list")?));
                break;
            }
            _ => {}
        }
        i += 1;
    }
    let quoted = format!("\"{source}\"");
    let mut out = text.to_string();
    match (array, disable) {
        (None, true) => {
            let root_open = toks[0].end;
            let has_members = toks.get(1).is_some_and(|t| t.kind != Kind::Punct('}'));
            let indent = member_indent(text, &toks);
            let comma = if has_members { "," } else { "" };
            out.insert_str(root_open, &format!("\n{indent}\"{KEY}\": [{quoted}]{comma}"));
        }
        (None, false) => return Ok(None),
        (Some((open, close)), true) => {
            let items: Vec<&Token> = toks[open + 1..close].iter().filter(|t| t.kind != Kind::Punct(',')).collect();
            match items.last() {
                Some(last) => out.insert_str(last.end, &format!(", {quoted}")),
                None => out.insert_str(toks[open].end, &quoted),
            }
        }
        (Some((open, close)), false)
            if toks[open + 1..close].iter().all(|t| string_is(text, t, source) || t.kind == Kind::Punct(',')) =>
        {
            // nothing else in the list: the whole member goes, with its
            // line and comma (undoing what adding it did)
            let key = open - 2;
            let (mut from, mut to) = (toks[key].start, toks[close].end);
            match (toks.get(close + 1), key.checked_sub(1).map(|p| toks[p])) {
                (Some(next), _) if next.kind == Kind::Punct(',') => to = next.end,
                (_, Some(prev)) if prev.kind == Kind::Punct(',') => from = prev.start,
                _ => {}
            }
            let line_start = text[..from].rfind('\n');
            if let Some(n) = line_start.filter(|&n| text[n + 1..from].trim().is_empty()) {
                from = if n > 0 && text.as_bytes()[n - 1] == b'\r' { n - 1 } else { n };
            }
            out.replace_range(from..to, "");
        }
        (Some((open, close)), false) => {
            // remove every matching entry, from the back
            let inner = &toks[open + 1..close];
            for (k, t) in inner.iter().enumerate().rev() {
                if !string_is(text, t, source) {
                    continue;
                }
                let (from, to) = match (inner.get(k + 1), k.checked_sub(1).and_then(|p| inner.get(p))) {
                    (Some(next), _) if next.kind == Kind::Punct(',') => {
                        let after = inner.get(k + 2).map_or(next.end, |n| n.start);
                        (t.start, after)
                    }
                    (_, Some(prev)) if prev.kind == Kind::Punct(',') => (prev.start, t.end),
                    _ => (t.start, t.end),
                };
                out.replace_range(from..to, "");
            }
        }
    }
    // the result must still read, and say what was asked
    if jsonc::parse(&out).is_err() || is_disabled(&out, source) != disable {
        return Err("settings.json: the change didn't come out right; nothing written".into());
    }
    Ok(Some(out))
}

/// Change the file: a copy goes to `backup_dir` first, then the new text
/// replaces the file. Returns the backup, or `None` if nothing changed.
pub fn set_disabled(settings: &Path, source: &str, disable: bool, backup_dir: &Path) -> io::Result<Option<PathBuf>> {
    let bytes = std::fs::read(settings)?;
    let text = String::from_utf8(bytes.clone()).map_err(|_| io::Error::other("settings.json isn't UTF-8"))?;
    let Some(new) = edit(&text, source, disable).map_err(io::Error::other)? else { return Ok(None) };
    std::fs::create_dir_all(backup_dir)?;
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let backup = backup_dir.join(format!("terminal-settings-{stamp}.json"));
    std::fs::write(&backup, &bytes)?;
    let temp = settings.with_extension("json.nativeterm-tmp");
    std::fs::write(&temp, new.as_bytes())?;
    std::fs::rename(&temp, settings)?;
    Ok(Some(backup))
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: &str = SSH_SOURCE;

    fn round(text: &str) -> (String, String) {
        let off = edit(text, S, true).unwrap().unwrap();
        assert!(is_disabled(&off, S));
        let on = edit(&off, S, false).unwrap().unwrap();
        assert!(!is_disabled(&on, S));
        (off, on)
    }

    #[test]
    fn adds_the_list_keeping_comments() {
        let text =
            "// my settings\n{\n    // theme\n    \"theme\": \"dark\",\n    \"profiles\": { \"list\": [] },\n}\n";
        let (off, on) = round(text);
        assert_eq!(
            off,
            "// my settings\n{\n    \"disabledProfileSources\": [\"Windows.Terminal.SSH\"],\n    // theme\n    \"theme\": \"dark\",\n    \"profiles\": { \"list\": [] },\n}\n"
        );
        assert_eq!(on, text, "turning it back on restores the file");
        assert_eq!(edit(&on, S, false).unwrap(), None, "already on");
    }

    #[test]
    fn existing_lists() {
        let text = "{\n\t\"disabledProfileSources\": [ \"Windows.Terminal.Azure\" /* cloud */ ],\n\t\"x\": 1\n}";
        let (off, on) = round(text);
        assert!(off.contains("[ \"Windows.Terminal.Azure\", \"Windows.Terminal.SSH\" /* cloud */ ]"), "{off}");
        assert_eq!(on, text);

        let text = "{\"disabledProfileSources\": [\"Windows.Terminal.SSH\", \"Windows.Terminal.Wsl\"]}";
        let on = edit(text, S, false).unwrap().unwrap();
        assert_eq!(on, "{\"disabledProfileSources\": [\"Windows.Terminal.Wsl\"]}");
        let text = "{\"disabledProfileSources\": [\"Windows.Terminal.Wsl\", \"Windows.Terminal.SSH\"]}";
        assert_eq!(edit(text, S, false).unwrap().unwrap(), "{\"disabledProfileSources\": [\"Windows.Terminal.Wsl\"]}");
        let text = "{\"disabledProfileSources\": []}";
        assert_eq!(edit(text, S, true).unwrap().unwrap(), "{\"disabledProfileSources\": [\"Windows.Terminal.SSH\"]}");
        assert_eq!(
            edit("{}", S, true).unwrap().unwrap(),
            "{\n    \"disabledProfileSources\": [\"Windows.Terminal.SSH\"]}"
        );
        assert_eq!(round("{}").1, "{}");
        let crlf = "{\r\n  \"a\": 1,\r\n  \"b\": 2\r\n}\r\n";
        assert_eq!(round(crlf).1, crlf);
        let last = "{\"x\": 1, \"disabledProfileSources\": [\"Windows.Terminal.SSH\"]}";
        assert_eq!(edit(last, S, false).unwrap().unwrap(), "{\"x\": 1}");
        let first = "{\"disabledProfileSources\": [\"Windows.Terminal.SSH\"], \"x\": 1}";
        assert_eq!(edit(first, S, false).unwrap().unwrap(), "{ \"x\": 1}");
    }

    #[test]
    fn nested_keys_and_strings_are_not_the_list() {
        let text = "{\"profiles\": {\"disabledProfileSources\": \"no\"}, \"a\": \"disabledProfileSources: [\"}";
        let off = edit(text, S, true).unwrap().unwrap();
        assert!(off.starts_with("{\n    \"disabledProfileSources\": [\"Windows.Terminal.SSH\"],"), "{off}");
        assert!(edit("{\"disabledProfileSources\": 3}", S, true).is_err());
        assert!(edit("[1]", S, true).is_err());
        assert!(edit("{ broken", S, true).is_err());
    }

    #[test]
    fn file_with_backup() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings.json");
        std::fs::write(&file, "{ \"theme\": \"dark\" }").unwrap();
        let backup = set_disabled(&file, S, true, &dir.path().join("backups")).unwrap().unwrap();
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), "{ \"theme\": \"dark\" }");
        assert!(is_disabled(&std::fs::read_to_string(&file).unwrap(), S));
        assert_eq!(set_disabled(&file, S, true, &dir.path().join("backups")).unwrap(), None);
        set_disabled(&file, S, false, &dir.path().join("backups")).unwrap().unwrap();
        assert!(!is_disabled(&std::fs::read_to_string(&file).unwrap(), S));
    }
}
