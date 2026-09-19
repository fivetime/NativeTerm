//! SecureCRT's button bars into the command library, as part of "Import
//! from SecureCRT": the send-string buttons become commands grouped by
//! their bar; everything else is listed in the preview with the reason.

use std::path::Path;

use native_term_app::commands::{self, Command, Library};
use native_term_app::import::Line;
use native_term_app::t;
use native_term_config::button_bar::{self, Converted, Why};

/// A button that becomes a command.
#[derive(Clone, Debug)]
pub struct Imported {
    pub command: Command,
}

/// Read the button bar file in SecureCRT's `config` folder: the commands
/// it gives, and preview lines about it appended to `lines`.
pub fn scan(config: &Path, lines: &mut Vec<Line>) -> Vec<Imported> {
    let Some(file) = button_bar::find(config) else { return Vec::new() };
    let bytes = match std::fs::read(&file) {
        Ok(b) => b,
        Err(e) => {
            lines.push(Line {
                text: t!("buttons-unreadable", error = e.to_string()),
                warning: true,
                details: Vec::new(),
            });
            return Vec::new();
        }
    };
    let text = String::from_utf8_lossy(&bytes);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let buttons = button_bar::parse(text);
    let mut imported = Vec::new();
    let mut skipped = Vec::new();
    let mut bars: Vec<&str> = Vec::new();
    for b in &buttons {
        if !bars.contains(&b.bar.as_str()) {
            bars.push(&b.bar);
        }
        match b.convert() {
            Converted::Command { text, enter } => imported
                .push(Imported { command: Command { name: b.label.clone(), text, enter, group: Some(b.bar.clone()) } }),
            Converted::Skipped(why) => {
                let reason = match why {
                    Why::Function(f) => t!("buttons-why-function", function = f.as_str()),
                    Why::Substitution(c) => {
                        let code = format!("\\{c}");
                        t!("buttons-why-substitution", code = code.as_str())
                    }
                    Why::Empty => t!("buttons-why-empty"),
                };
                skipped.push(format!("{} / {}: {reason}", b.bar, b.label));
            }
        }
    }
    let details =
        imported.iter().map(|i| format!("{} / {}", i.command.group.as_deref().unwrap_or(""), i.command.name)).collect();
    lines.push(Line {
        text: t!("buttons-found", commands = imported.len(), bars = bars.len(), file = file.display().to_string()),
        warning: false,
        details,
    });
    if !skipped.is_empty() {
        lines.push(Line { text: t!("buttons-skipped", count = skipped.len()), warning: true, details: skipped });
    }
    imported
}

/// Add them to the library at `path`; the line for the result.
pub fn add(path: &Path, imported: Vec<Imported>) -> String {
    let mut library = match Library::load(path) {
        Ok(l) => l,
        Err(e) => return t!("buttons-library-failed", error = e.to_string()),
    };
    let merged = commands::merge(&mut library, imported.into_iter().map(|i| i.command).collect());
    if merged.added > 0 {
        if let Err(e) = library.save() {
            return t!("buttons-library-failed", error = e.to_string());
        }
    }
    t!("buttons-added", added = merged.added, present = merged.present, renamed = merged.renamed.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A made-up SecureCRT config: two bars, a send string with a pause and
    /// a menu button left out, one command whose name the library has.
    #[test]
    fn button_bars_into_the_library() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("Config");
        std::fs::create_dir_all(config.join("Sessions")).unwrap();
        std::fs::write(
            config.join("ButtonBarV5.ini"),
            "\u{feff}Z:\"Cisco\"=00000002\n SEND,show ip interface brief\\\\r,ip br,,,0,4,\n \
             SEND,en\\\\r\\\\pPASSWORD\\\\r,enable,,,0,5,\nZ:\"Linux\"=00000002\n SEND,df -h\\\\r,disk,,,0,1,\n \
             MENU_TAB_CLONE,,clone,,,0,0,\n",
        )
        .unwrap();
        let mut lines = Vec::new();
        let imported = scan(&config.join("Sessions"), &mut lines);
        let names: Vec<&str> = imported.iter().map(|i| i.command.name.as_str()).collect();
        assert_eq!(names, ["ip br", "disk"]);
        assert_eq!(lines.len(), 2, "found, and left out");
        assert_eq!(lines[1].details.len(), 2, "enable (a pause) and clone (a menu function)");
        assert!(
            !lines[1].details.iter().any(|d| d.contains("PASSWORD")),
            "the reason, not the text: {:?}",
            lines[1].details
        );

        let library = dir.path().join("commands.toml");
        std::fs::write(&library, "[[command]]\nname = \"disk\"\ntext = \"df -hT\"\n").unwrap();
        add(&library, imported.clone());
        let saved = Library::load(&library).unwrap();
        let names: Vec<(&str, Option<&str>)> =
            saved.commands.iter().map(|c| (c.name.as_str(), c.group.as_deref())).collect();
        assert_eq!(names, [("disk", None), ("ip br", Some("Cisco")), ("disk (Linux)", Some("Linux"))]);
        // again: nothing new
        add(&library, imported);
        assert_eq!(Library::load(&library).unwrap().commands.len(), 3);
    }
}
