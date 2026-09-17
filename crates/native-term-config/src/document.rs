//! A line-preserving model of one ssh_config file.
//!
//! Every line is kept verbatim; edits replace or insert whole lines, so
//! comments, spacing, and keyword spelling written by the user survive
//! (never a parse-and-reserialize round trip).

/// Line ending used when the file is written back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Eol {
    Lf,
    CrLf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Directive {
    /// Keyword as written (case preserved; ssh compares case-insensitively).
    pub keyword: String,
    /// Arguments after ssh-style unquoting.
    pub args: Vec<String>,
    /// Byte offset in the line where the value starts.
    value_start: usize,
}

impl Directive {
    pub fn is(&self, keyword: &str) -> bool {
        self.keyword.eq_ignore_ascii_case(keyword)
    }

    /// The value of a single-valued keyword: arguments joined by one space.
    pub fn value(&self) -> String {
        self.args.join(" ")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LineKind {
    Blank,
    Comment,
    Directive(Directive),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub text: String,
    pub kind: LineKind,
}

impl Line {
    pub fn directive(&self) -> Option<&Directive> {
        match &self.kind {
            LineKind::Directive(d) => Some(d),
            _ => None,
        }
    }

    /// A directive's value as written (quotes kept), for keywords that are
    /// edited verbatim.
    pub fn raw_value(&self) -> Option<&str> {
        self.directive().map(|d| self.text[d.value_start..].trim_end())
    }

    fn indent(&self) -> &str {
        &self.text[..self.text.len() - self.text.trim_start().len()]
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockKind {
    /// Lines before the first `Host`/`Match`: they apply to every host.
    Global,
    /// `Host` with its pattern list.
    Host(Vec<String>),
    /// `Match` with its raw criteria.
    Match(String),
}

/// A section of the file: the global part, or one `Host`/`Match` block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub kind: BlockKind,
    /// Line index of the `Host`/`Match` header (`None` for the global part).
    pub header: Option<usize>,
    /// Lines `[start, end)`, header included.
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Document {
    pub lines: Vec<Line>,
    pub eol: Eol,
    pub trailing_newline: bool,
}

impl Document {
    pub fn parse(text: &str) -> Document {
        let eol = if text.contains("\r\n") { Eol::CrLf } else { Eol::Lf };
        let trailing_newline = text.ends_with('\n');
        let body = text.strip_suffix('\n').unwrap_or(text);
        let lines = if text.is_empty() {
            Vec::new()
        } else {
            body.split('\n').map(|l| parse_line(l.strip_suffix('\r').unwrap_or(l))).collect()
        };
        Document { lines, eol, trailing_newline }
    }

    pub fn render(&self) -> String {
        let nl = match self.eol {
            Eol::Lf => "\n",
            Eol::CrLf => "\r\n",
        };
        let mut out = self.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join(nl);
        if self.trailing_newline && !self.lines.is_empty() {
            out.push_str(nl);
        }
        out
    }

    pub fn blocks(&self) -> Vec<Block> {
        let mut blocks = Vec::new();
        let mut current = Block { kind: BlockKind::Global, header: None, start: 0, end: 0 };
        for (i, line) in self.lines.iter().enumerate() {
            let Some(d) = line.directive() else { continue };
            let kind = if d.is("Host") {
                BlockKind::Host(d.args.clone())
            } else if d.is("Match") {
                BlockKind::Match(d.value())
            } else {
                continue;
            };
            current.end = i;
            blocks.push(current);
            current = Block { kind, header: Some(i), start: i, end: 0 };
        }
        current.end = self.lines.len();
        blocks.push(current);
        blocks
    }

    /// Index (into [`Document::blocks`]) of the first `Host` block listing
    /// `alias` literally.
    pub fn find_host_block(&self, alias: &str) -> Option<usize> {
        self.blocks().iter().position(|b| match &b.kind {
            BlockKind::Host(patterns) => patterns.iter().any(|p| p.eq_ignore_ascii_case(alias)),
            _ => false,
        })
    }

    /// Directives inside a block, header excluded, with their line index.
    pub fn directives<'a>(&'a self, block: &Block) -> impl Iterator<Item = (usize, &'a Directive)> + 'a {
        let first = block.header.map_or(block.start, |h| h + 1);
        (first..block.end).filter_map(move |i| self.lines[i].directive().map(|d| (i, d)))
    }

    /// First value wins in ssh, so this is the effective one within the block.
    pub fn get(&self, block: usize, keyword: &str) -> Option<&Directive> {
        let b = self.blocks().swap_remove(block);
        self.directives(&b).map(|(_, d)| d).find(|d| d.is(keyword))
    }

    pub fn get_all(&self, block: usize, keyword: &str) -> Vec<&Directive> {
        let b = self.blocks().swap_remove(block);
        self.directives(&b).map(|(_, d)| d).filter(|d| d.is(keyword)).collect()
    }

    /// Set a single-valued keyword: replace the value of its first occurrence
    /// in place (indent, spelling, and separator kept), or add a line after
    /// the block's last directive.
    pub fn set(&mut self, block: usize, keyword: &str, value: &str) {
        let b = self.blocks().swap_remove(block);
        let existing = self.directives(&b).find(|(_, d)| d.is(keyword)).map(|(i, d)| (i, d.value_start));
        match existing {
            Some((i, value_start)) => {
                let line = &self.lines[i].text;
                let mut text = line[..value_start].to_string();
                if !text.ends_with(|c: char| c.is_whitespace() || c == '=') {
                    text.push(' ');
                }
                text.push_str(&quote_arg(value));
                self.lines[i] = parse_line(&text);
            }
            None => {
                let (at, indent) = self.insertion_point(&b);
                self.lines.insert(at, parse_line(&format!("{indent}{keyword} {}", quote_arg(value))));
            }
        }
    }

    /// Set the values of a keyword that may repeat, each written verbatim:
    /// existing lines are rewritten in place, extra ones removed, new ones
    /// added after the block's last directive.
    pub fn set_raw_values(&mut self, block: usize, keyword: &str, values: &[String]) {
        let b = self.blocks().swap_remove(block);
        let existing: Vec<(usize, usize)> =
            self.directives(&b).filter(|(_, d)| d.is(keyword)).map(|(i, d)| (i, d.value_start)).collect();
        for ((i, value_start), value) in existing.iter().zip(values) {
            let mut text = self.lines[*i].text[..*value_start].to_string();
            if !text.ends_with(|c: char| c.is_whitespace() || c == '=') {
                text.push(' ');
            }
            text.push_str(value);
            self.lines[*i] = parse_line(&text);
        }
        for (i, _) in existing.iter().skip(values.len()).rev() {
            self.lines.remove(*i);
        }
        if values.len() > existing.len() {
            let b = self.blocks().swap_remove(block);
            let (mut at, indent) = self.insertion_point(&b);
            for value in &values[existing.len()..] {
                self.lines.insert(at, parse_line(&format!("{indent}{keyword} {value}")));
                at += 1;
            }
        }
    }

    /// Remove every occurrence of a keyword in a block; returns how many.
    pub fn remove(&mut self, block: usize, keyword: &str) -> usize {
        let b = self.blocks().swap_remove(block);
        let doomed: Vec<usize> = self.directives(&b).filter(|(_, d)| d.is(keyword)).map(|(i, _)| i).collect();
        for i in doomed.iter().rev() {
            self.lines.remove(*i);
        }
        doomed.len()
    }

    /// Insert a raw directive line at the start of the global part (after
    /// leading comments), e.g. for `IgnoreUnknown`.
    pub fn insert_global_first(&mut self, keyword: &str, value: &str) -> usize {
        let at = self
            .lines
            .iter()
            .position(|l| !matches!(l.kind, LineKind::Comment))
            .unwrap_or(self.lines.len());
        self.lines.insert(at, parse_line(&format!("{keyword} {}", quote_arg(value))));
        at
    }

    pub fn insert_line(&mut self, at: usize, keyword: &str, value: &str) {
        self.lines.insert(at, parse_line(&format!("{keyword} {}", quote_arg(value))));
    }

    /// Append a `Host` block at the end of the file.
    pub fn append_host(&mut self, patterns: &[&str], entries: &[(&str, &str)]) {
        if self.lines.last().is_some_and(|l| l.kind != LineKind::Blank) {
            self.lines.push(parse_line(""));
        }
        let header = patterns.iter().map(|p| quote_arg(p)).collect::<Vec<_>>().join(" ");
        self.lines.push(parse_line(&format!("Host {header}")));
        for (k, v) in entries {
            self.lines.push(parse_line(&format!("    {k} {}", quote_arg(v))));
        }
        self.trailing_newline = true;
    }

    /// Remove a whole `Host`/`Match` block, and one blank line in front of
    /// it so no double gap is left behind.
    pub fn remove_block(&mut self, block: usize) {
        let b = self.blocks().swap_remove(block);
        if b.header.is_none() {
            return;
        }
        let mut start = b.start;
        if start > 0 && self.lines[start - 1].kind == LineKind::Blank {
            start -= 1;
        }
        self.lines.drain(start..b.end);
    }

    fn insertion_point(&self, b: &Block) -> (usize, String) {
        let mut dirs = self.directives(b).peekable();
        let indent = match dirs.peek() {
            Some((i, _)) => self.lines[*i].indent().to_string(),
            None if b.header.is_some() => "    ".to_string(),
            None => String::new(),
        };
        let last = self.directives(b).map(|(i, _)| i).last();
        let at = match (last, b.header) {
            (Some(i), _) => i + 1,
            (None, Some(h)) => h + 1,
            (None, None) => 0,
        };
        (at, indent)
    }
}

pub(crate) fn parse_line(text: &str) -> Line {
    let trimmed = text.trim_start();
    let kind = if trimmed.is_empty() {
        LineKind::Blank
    } else if trimmed.starts_with('#') {
        LineKind::Comment
    } else {
        let kw_len = trimmed.find(|c: char| c.is_whitespace() || c == '=').unwrap_or(trimmed.len());
        let rest = trimmed[kw_len..].trim_start();
        let rest = rest.strip_prefix('=').map(str::trim_start).unwrap_or(rest);
        LineKind::Directive(Directive {
            keyword: trimmed[..kw_len].to_string(),
            args: split_args(rest.trim_end()),
            value_start: text.len() - rest.len(),
        })
    };
    Line { text: text.to_string(), kind }
}

/// ssh-style argument splitting: whitespace separates, single or double
/// quotes group, backslash escapes `\`, quotes, and space.
pub(crate) fn split_args(s: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_arg = false;
    let mut quote: Option<char> = None;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (_, '\\') if matches!(chars.peek(), Some('\\' | '"' | '\'' | ' ')) => {
                current.push(chars.next().unwrap_or('\\'));
                in_arg = true;
            }
            (None, c) if c.is_whitespace() => {
                if in_arg {
                    args.push(std::mem::take(&mut current));
                    in_arg = false;
                }
            }
            (None, '"' | '\'') => {
                quote = Some(c);
                in_arg = true;
            }
            (Some(q), c) if c == q => quote = None,
            (_, c) => {
                current.push(c);
                in_arg = true;
            }
        }
    }
    if in_arg {
        args.push(current);
    }
    args
}

/// Quote a value for writing when ssh would otherwise split or misread it.
pub(crate) fn quote_arg(value: &str) -> String {
    let plain = !value.is_empty() && !value.chars().any(|c| c.is_whitespace() || matches!(c, '"' | '\'' | '\\' | '#'));
    if plain {
        value.to_string()
    } else {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# my hosts\r\nHost node01 incus-node-01\r\n  HostName 10.32.32.130\r\n  User=root\r\n\r\nHost *\r\n    ServerAliveInterval 30\r\n";

    #[test]
    fn round_trip_is_byte_identical() {
        for text in [SAMPLE, "", "\n", "Host a\n  User x", "Host a\n\n\n# end\n"] {
            assert_eq!(Document::parse(text).render(), text, "{text:?}");
        }
    }

    #[test]
    fn parses_keywords_values_and_blocks() {
        let doc = Document::parse(SAMPLE);
        let blocks = doc.blocks();
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].kind, BlockKind::Global);
        assert_eq!(blocks[1].kind, BlockKind::Host(vec!["node01".into(), "incus-node-01".into()]));
        assert_eq!(doc.get(1, "hostname").unwrap().value(), "10.32.32.130");
        assert_eq!(doc.get(1, "USER").unwrap().value(), "root");
        assert!(doc.get(1, "ServerAliveInterval").is_none(), "not visible across blocks");
        assert_eq!(doc.find_host_block("INCUS-NODE-01"), Some(1));
    }

    #[test]
    fn splits_quoted_arguments() {
        assert_eq!(split_args(r#"a "b c" 'd e' f\ g"#), vec!["a", "b c", "d e", "f g"]);
        assert_eq!(split_args(r#""10.32.16.66(osp control1)""#), vec!["10.32.16.66(osp control1)"]);
        assert_eq!(split_args(r#""say \"hi\"""#), vec![r#"say "hi""#]);
        assert!(split_args("   ").is_empty());
    }

    #[test]
    fn set_replaces_in_place_and_keeps_formatting() {
        let mut doc = Document::parse(SAMPLE);
        doc.set(1, "user", "admin");
        doc.set(1, "HostName", "控制节点 1");
        let text = doc.render();
        assert!(text.contains("  User=admin\r\n"), "{text}");
        assert!(text.contains("  HostName \"控制节点 1\"\r\n"), "{text}");
        assert_eq!(doc.get(1, "hostname").unwrap().value(), "控制节点 1");
    }

    #[test]
    fn set_inserts_after_last_directive_with_block_indent() {
        let mut doc = Document::parse(SAMPLE);
        doc.set(1, "NativeTermLabel", "node01 (incus)");
        let text = doc.render();
        let lines: Vec<&str> = text.split("\r\n").collect();
        assert_eq!(lines[4], "  NativeTermLabel \"node01 (incus)\"");
        assert_eq!(lines[5], "", "blank separator stays after the block");
    }

    #[test]
    fn set_on_bare_keyword_adds_a_separator() {
        let mut doc = Document::parse("Host a\n  User\n");
        doc.set(1, "User", "x");
        assert_eq!(doc.render(), "Host a\n  User x\n");
    }

    #[test]
    fn remove_and_remove_block() {
        let mut doc = Document::parse(SAMPLE);
        assert_eq!(doc.remove(1, "user"), 1);
        doc.remove_block(2);
        assert_eq!(doc.render(), "# my hosts\r\nHost node01 incus-node-01\r\n  HostName 10.32.32.130\r\n");
    }

    #[test]
    fn append_host_block() {
        let mut doc = Document::parse("Host a\n  User x");
        doc.append_host(&["web01"], &[("HostName", "10.0.0.1"), ("NativeTermLabel", "web 01")]);
        assert_eq!(
            doc.render(),
            "Host a\n  User x\n\nHost web01\n    HostName 10.0.0.1\n    NativeTermLabel \"web 01\"\n"
        );
    }
}
