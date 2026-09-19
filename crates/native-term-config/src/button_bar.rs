//! SecureCRT's button bars (`ButtonBarV5.ini`, older `ButtonBarV4.ini`, in
//! its config folder next to `Sessions`), for the command library. The
//! format, as VanDyke's "How to Import a Single Button Bar" shows it:
//!
//! ```text
//! Z:"Keyword HL Video BBar"=00000007
//!  SEND,sh ip int br\\r,ip br,,,0,4,
//!  MENU_TOGGLE_KEYWORD_HIGHLIGHTING,,Highlight on/off,,,0,0,
//! ```
//!
//! A bar's name with its button count (hex), then one line per button
//! with a leading space: the function, its argument, the label, and four
//! more fields. Backslashes are doubled in the file; the argument of a
//! `SEND` button is then a send string with SecureCRT's escapes (`\r`,
//! `\n`, `\\`, `\t`, `\p`, `\u`, …). Only send strings become commands:
//! scripts, menu functions and programs have no counterpart.

use std::path::{Path, PathBuf};

/// A button as the file has it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Button {
    pub bar: String,
    pub label: String,
    /// `SEND`, `MENU_…`, `SCRIPT`, …
    pub function: String,
    /// The argument with the file's doubled backslashes undone.
    pub argument: String,
}

/// A button as a command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Converted {
    /// Lines separated by `\n`; `enter`: Enter after the last one.
    Command {
        text: String,
        enter: bool,
    },
    Skipped(Why),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Why {
    /// Not a send string (a script, a menu function, a program, …).
    Function(String),
    /// Uses something SecureCRT fills in or does itself (`\p` pause,
    /// `\u` user name, `\v` clipboard, local addresses, …).
    Substitution(char),
    Empty,
}

/// The button bar file in `config` (SecureCRT's "Config Path"; its
/// `Sessions` folder is accepted too).
pub fn find(config: &Path) -> Option<PathBuf> {
    let config =
        if config.file_name().is_some_and(|n| n.eq_ignore_ascii_case("Sessions")) { config.parent()? } else { config };
    ["ButtonBarV5.ini", "ButtonBarV4.ini"].iter().map(|n| config.join(n)).find(|p| p.is_file())
}

/// The buttons of every bar in the file's text.
pub fn parse(text: &str) -> Vec<Button> {
    let mut buttons = Vec::new();
    let mut bar: Option<String> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Z:\"") {
            bar = rest.split_once("\"=").map(|(name, _)| name.to_string());
            continue;
        }
        let (Some(bar), Some(body)) = (&bar, line.strip_prefix(' ')) else { continue };
        // the function, the argument (which may hold commas), the label and
        // four fields: read from both ends
        let fields: Vec<&str> = body.split(',').collect();
        if fields.len() < 7 {
            continue;
        }
        let label_at = fields.len() - 6;
        buttons.push(Button {
            bar: bar.clone(),
            label: fields[label_at].to_string(),
            function: fields[0].to_string(),
            argument: fields[1..label_at].join(",").replace("\\\\", "\\"),
        });
    }
    buttons
}

impl Button {
    /// What the command library gets.
    pub fn convert(&self) -> Converted {
        if !self.function.eq_ignore_ascii_case("SEND") {
            return Converted::Skipped(Why::Function(self.function.clone()));
        }
        let mut text = String::new();
        let mut chars = self.argument.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '\\' {
                text.push(c);
                continue;
            }
            match chars.next() {
                Some('r' | 'n') => text.push('\n'),
                Some('\\') => text.push('\\'),
                Some('t') => text.push('\t'),
                Some(d @ '0'..='7') => {
                    // \nnn: an octal code
                    let mut code = d.to_digit(8).unwrap_or(0);
                    for _ in 0..2 {
                        match chars.peek().and_then(|c| c.to_digit(8)) {
                            Some(v) => {
                                code = code * 8 + v;
                                chars.next();
                            }
                            None => break,
                        }
                    }
                    match char::from_u32(code) {
                        Some(ch) if !ch.is_control() || ch == '\t' => text.push(ch),
                        Some('\r' | '\n') => text.push('\n'),
                        _ => return Converted::Skipped(Why::Substitution(d)),
                    }
                }
                Some(other) => return Converted::Skipped(Why::Substitution(other)),
                None => text.push('\\'),
            }
        }
        // "\r" at the end: Enter after the last line
        let enter = text.ends_with('\n');
        let text = text.trim_end_matches('\n').to_string();
        if text.trim().is_empty() {
            return Converted::Skipped(Why::Empty);
        }
        Converted::Command { text, enter }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// VanDyke's own example, as its web page prints it.
    const EXAMPLE: &str = "Z:\"Keyword HL Video BBar\"=00000007\n \
        SEND,sh ip int br\\\\r,ip br,,,0,4,\n \
        SEND,ena\\\\r\\\\pYOUR_ENABLE_PASSWORD_HERE\\\\r,enable,,,0,5,\n \
        SEND,configure terminal\\\\r,conf t,,,0,7,\n \
        SEND,ip dhcp pool dhcp-clients\\\\r,dhcp,,,0,2,\n \
        SEND,exit\\\\r,exit,,,0,10,\n \
        SEND,disable\\\\r,disable,,,0,1,\n \
        MENU_TOGGLE_KEYWORD_HIGHLIGHTING,,Highlight on/off,,,0,0,\n";

    #[test]
    fn vandykes_example() {
        let buttons = parse(EXAMPLE);
        assert_eq!(buttons.len(), 7);
        assert_eq!(buttons[0].bar, "Keyword HL Video BBar");
        assert_eq!(buttons[0].label, "ip br");
        assert_eq!(buttons[0].argument, "sh ip int br\\r");
        assert_eq!(buttons[0].convert(), Converted::Command { text: "sh ip int br".into(), enter: true });
        // the enable password with a pause: not something to copy blindly
        assert_eq!(buttons[1].convert(), Converted::Skipped(Why::Substitution('p')));
        assert_eq!(buttons[6].label, "Highlight on/off");
        assert_eq!(buttons[6].convert(), Converted::Skipped(Why::Function("MENU_TOGGLE_KEYWORD_HIGHLIGHTING".into())));
    }

    #[test]
    fn send_strings() {
        let button = |argument: &str| Button {
            bar: "b".into(),
            label: "l".into(),
            function: "SEND".into(),
            argument: argument.into(),
        };
        // several lines; no \r at the end: the last one is left to finish
        assert_eq!(
            button("cd /srv\\rls -la").convert(),
            Converted::Command { text: "cd /srv\nls -la".into(), enter: false }
        );
        assert_eq!(button("echo a\\\\b\\r").convert(), Converted::Command { text: "echo a\\b".into(), enter: true });
        assert_eq!(button("x\\134y\\r").convert(), Converted::Command { text: "x\\y".into(), enter: true }, "octal");
        assert_eq!(button("ssh \\u@host\\r").convert(), Converted::Skipped(Why::Substitution('u')));
        assert_eq!(button("\\r").convert(), Converted::Skipped(Why::Empty));
    }

    #[test]
    fn commas_in_the_command_and_other_bars() {
        let text = "Z:\"A\"=00000001\n SEND,awk -F, '{print $1}'\\\\r,awk,,,0,1,\nZ:\"B\"=00000001\n SEND,uptime\\\\r,up,,,0,1,\n";
        let buttons = parse(text);
        assert_eq!(buttons.len(), 2);
        assert_eq!(buttons[0].argument, "awk -F, '{print $1}'\\r");
        assert_eq!(buttons[0].label, "awk");
        assert_eq!(buttons[1].bar, "B");
    }
}
