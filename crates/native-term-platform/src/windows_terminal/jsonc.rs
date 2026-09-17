//! Windows Terminal's `settings.json` is JSON with comments and trailing
//! commas. Read-only: comments are dropped, so this is never written back.

use serde_json::Value;

/// Parse JSON that may contain `//` and `/* */` comments and trailing commas.
pub fn parse(text: &str) -> serde_json::Result<Value> {
    serde_json::from_str(&drop_trailing_commas(&drop_comments(text.trim_start_matches('\u{feff}'))))
}

/// Calls `f` for every character with whether it is inside a string.
fn scan(text: &str, mut f: impl FnMut(usize, char, bool)) {
    let mut in_string = false;
    let mut escaped = false;
    for (i, c) in text.char_indices() {
        let inside = in_string;
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
        } else if c == '"' {
            in_string = true;
        }
        f(i, c, inside || c == '"');
    }
}

fn drop_comments(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    // end of the comment being skipped, if any
    let mut skip_until: Option<usize> = None;
    scan(text, |i, c, string| {
        if let Some(end) = skip_until {
            if i < end {
                return;
            }
            skip_until = None;
        }
        if !string && c == '/' {
            match bytes.get(i + 1) {
                Some(b'/') => {
                    skip_until = Some(text[i..].find('\n').map_or(text.len(), |n| i + n));
                    return;
                }
                Some(b'*') => {
                    skip_until = Some(text[i + 2..].find("*/").map_or(text.len(), |n| i + 2 + n + 2));
                    return;
                }
                _ => {}
            }
        }
        out.push(c);
    });
    out
}

fn drop_trailing_commas(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    scan(text, |i, c, string| {
        if !string && c == ',' {
            let next = text[i + 1..].trim_start().chars().next();
            if matches!(next, Some('}') | Some(']')) {
                return;
            }
        }
        out.push(c);
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_and_trailing_commas() {
        let text = "\u{feff}{\n  // a comment\n  \"a\": \"x // not a comment\", /* block, */\n  \"b\": [1, 2, // last\n ],\n  \"c\": \"quote \\\" , ]\",\n  \"d\": \"back\\\\\",\n}\n";
        let v = parse(text).unwrap();
        assert_eq!(v["a"], "x // not a comment");
        assert_eq!(v["b"], serde_json::json!([1, 2]));
        assert_eq!(v["c"], "quote \" , ]");
        assert_eq!(v["d"], "back\\");
    }

    #[test]
    fn plain_json_unchanged() {
        let v = parse(r#"{"profiles": {"list": [{"name": "a,b"}]}}"#).unwrap();
        assert_eq!(v["profiles"]["list"][0]["name"], "a,b");
    }

    #[test]
    fn large_file() {
        let items: Vec<String> = (0..20_000).map(|n| format!("{{\"n\": {n}}}, // item\n")).collect();
        let text = format!("{{\"list\": [{}]}}", items.concat());
        assert_eq!(parse(&text).unwrap()["list"].as_array().unwrap().len(), 20_000);
    }
}
