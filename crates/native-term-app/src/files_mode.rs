//! How files go across: as they are (binary), as text (line ends as the
//! side copied to has them: CR LF on Windows, LF on the server), or
//! chosen by the file (Auto: known text and known binary extensions;
//! for one not known, the person is asked, as SecureCRT asks: "Choose
//! Transfer Type", ASCII or Binary, for this extension always, for every
//! unknown one in this transfer). SFTP itself has no text mode: the
//! line ends are changed here. Where both sides end lines alike (a Linux
//! or macOS computer), text is copied as it is and nothing is asked.

use super::*;

const MODE_KEY: &str = "files.transfer_mode";
const TYPES_KEY: &str = "files.transfer_types";

/// The transfer mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(super) enum Mode {
    #[default]
    Auto,
    Binary,
    Text,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Auto, Mode::Binary, Mode::Text];

    pub fn label(self) -> String {
        match self {
            Mode::Auto => t!("files-mode-auto"),
            Mode::Binary => t!("files-mode-binary"),
            Mode::Text => t!("files-mode-text"),
        }
    }

    fn key(self) -> &'static str {
        match self {
            Mode::Auto => "auto",
            Mode::Binary => "binary",
            Mode::Text => "text",
        }
    }

    pub fn read(core: Option<&native_term_app::Core>) -> Mode {
        match core.and_then(|c| c.setting(MODE_KEY)).as_deref() {
            Some("binary") => Mode::Binary,
            Some("text") => Mode::Text,
            _ => Mode::Auto,
        }
    }

    pub fn keep(self, core: Option<&native_term_app::Core>) {
        if let Some(core) = core {
            core.set_setting(MODE_KEY, self.key());
        }
    }
}

/// Whether text has to change its line ends to go across: this computer
/// ends them CR LF (Windows), the server LF.
pub(super) const LINES_DIFFER: bool = cfg!(windows);

/// What an extension (lowercase, without the dot) is known as: text,
/// binary, or not known (`None`).
fn known(ext: &str) -> Option<bool> {
    const TEXT: &[&str] = &[
        "txt",
        "text",
        "md",
        "markdown",
        "rst",
        "log",
        "csv",
        "tsv",
        "ini",
        "cfg",
        "conf",
        "config",
        "cnf",
        "properties",
        "env",
        "yaml",
        "yml",
        "toml",
        "json",
        "xml",
        "html",
        "htm",
        "css",
        "scss",
        "js",
        "mjs",
        "ts",
        "jsx",
        "tsx",
        "sh",
        "bash",
        "zsh",
        "ksh",
        "fish",
        "ps1",
        "bat",
        "cmd",
        "py",
        "pl",
        "pm",
        "rb",
        "php",
        "lua",
        "go",
        "rs",
        "c",
        "h",
        "cc",
        "cpp",
        "hpp",
        "cs",
        "java",
        "kt",
        "swift",
        "sql",
        "service",
        "timer",
        "socket",
        "repo",
        "list",
        "spec",
        "patch",
        "diff",
        "tf",
        "hcl",
        "j2",
        "tpl",
        "vim",
        "gitignore",
        "dockerfile",
        "mk",
        "cmake",
        "gradle",
        "sed",
        "awk",
    ];
    const BINARY: &[&str] = &[
        "zip", "gz", "tgz", "bz2", "xz", "zst", "7z", "rar", "tar", "iso", "img", "qcow2", "vmdk", "vdi", "raw", "bin",
        "exe", "dll", "so", "a", "o", "ko", "deb", "rpm", "apk", "msi", "jar", "war", "class", "pyc", "png", "jpg",
        "jpeg", "gif", "bmp", "ico", "webp", "tif", "tiff", "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt",
        "mp3", "mp4", "mkv", "avi", "mov", "wav", "flac", "ogg", "ttf", "otf", "woff", "woff2", "db", "sqlite", "pcap",
        "pem", "der", "p12", "pfx", "gpg", "sig",
    ];
    if TEXT.contains(&ext) {
        Some(true)
    } else if BINARY.contains(&ext) {
        Some(false)
    } else {
        None
    }
}

/// A file name's extension, lowercase (the whole name for a dot file:
/// `.gitignore` is `gitignore`); empty without one.
pub(super) fn extension(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((_, ext)) if !ext.is_empty() => ext.to_lowercase(),
        _ => String::new(),
    }
}

/// The extensions the person chose a type for, for always (`state.db`, a
/// line each: extension, a tab, `text` or `binary`).
pub(super) fn chosen_types(core: Option<&native_term_app::Core>) -> HashMap<String, bool> {
    core.and_then(|c| c.setting(TYPES_KEY))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_once('\t'))
        .map(|(ext, kind)| (ext.to_string(), kind == "text"))
        .collect()
}

fn keep_type(core: Option<&native_term_app::Core>, ext: &str, text: bool) {
    let Some(core) = core else { return };
    let mut types = chosen_types(Some(core));
    types.insert(ext.to_string(), text);
    let mut lines: Vec<String> =
        types.iter().map(|(e, t)| format!("{e}\t{}", if *t { "text" } else { "binary" })).collect();
    lines.sort();
    core.set_setting(TYPES_KEY, &lines.join("\n"));
}

/// The person's answer to "Choose Transfer Type".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Answer {
    pub text: bool,
    /// For this extension always.
    pub always: bool,
    /// For every file not known in this transfer.
    pub all: bool,
}

/// Decides which planned files go as text. `ask` is asked about a file
/// not known (its name, its extension); `None` from it cancels.
pub(super) fn decide(
    items: &mut [transfer::Item],
    mode: Mode,
    mut types: HashMap<String, bool>,
    mut ask: impl FnMut(&str, &str) -> Option<Answer>,
) -> Result<(), native_term_sftp::Error> {
    let mut all: Option<bool> = None;
    for item in items.iter_mut().filter(|i| !i.dir) {
        item.text = match mode {
            _ if !LINES_DIFFER => false,
            Mode::Binary => false,
            Mode::Text => true,
            Mode::Auto => {
                let name = item.local.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let ext = extension(&name);
                match types.get(&ext).copied().or_else(|| known(&ext)).or(all) {
                    Some(text) => text,
                    None => {
                        let answer = ask(&name, &ext).ok_or(native_term_sftp::Error::Cancelled)?;
                        if answer.always {
                            types.insert(ext, answer.text);
                        }
                        if answer.all {
                            all = Some(answer.text);
                        }
                        answer.text
                    }
                }
            }
        };
    }
    Ok(())
}

/// The question as the window holds it while it is asked.
pub(super) struct TypeQuestion {
    pub name: String,
    pub ext: String,
    pub reply: Sender<Option<Answer>>,
    pub text: bool,
    pub always: bool,
    pub all: bool,
}

impl FilesWindow {
    /// The status line's transfer mode: a menu to change it.
    pub(super) fn mode_menu(&mut self, ui: &mut egui::Ui, palette: &native_term_skin::Palette) {
        let core = self.core().cloned();
        let mode = Mode::read(core.as_ref());
        let small = egui::FontId::proportional(11.0);
        let text = format!("{} {} {}", t!("files-mode"), mode.label(), icons::CHEVRON_DOWN);
        let response = ui
            .add(
                egui::Label::new(egui::RichText::new(text).font(small).color(palette.weak)).sense(egui::Sense::click()),
            )
            .on_hover_text(t!("files-mode-hint"));
        egui::Popup::menu(&response).show(|ui| {
            for m in Mode::ALL {
                if ui.radio(m == mode, m.label()).clicked() {
                    m.keep(core.as_ref());
                    ui.close();
                }
            }
        });
    }

    /// "Choose Transfer Type" (a window of its own, modal), as SecureCRT
    /// has it.
    pub(super) fn type_dialog(&mut self, ctx: &egui::Context) {
        let Some(q) = &mut self.ask_type else { return };
        let mut done = None;
        let closed = modal(ctx, "files-type", t!("files-type-title"), crate::icons::DOCUMENT, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(icons::DOCUMENT.to_string()).size(28.0));
                ui.label(egui::RichText::new(&q.name).size(14.0));
            });
            ui.separator();
            ui.label(t!("files-type-ask"));
            ui.radio_value(&mut q.text, true, t!("files-type-ascii"));
            ui.radio_value(&mut q.text, false, t!("files-type-binary"));
            ui.add_space(8.0);
            ui.add_enabled(!q.ext.is_empty(), egui::Checkbox::new(&mut q.always, t!("files-type-always")));
            ui.checkbox(&mut q.all, t!("files-type-all"));
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            done = ok_cancel(ui, t!("button-ok"), native_term_skin::Role::Primary).or(enter.then_some(true));
        });
        if closed && done.is_none() {
            done = Some(false);
        }
        let Some(ok) = done else { return };
        let Some(q) = self.ask_type.take() else { return };
        let answer = ok.then_some(Answer { text: q.text, always: q.always && !q.ext.is_empty(), all: q.all });
        if let Some(a) = answer.filter(|a| a.always) {
            keep_type(self.core(), &q.ext, a.text);
        }
        let _ = q.reply.send(answer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str) -> transfer::Item {
        transfer::Item {
            remote: name.as_bytes().to_vec(),
            local: PathBuf::from(name),
            dir: false,
            size: 1,
            permissions: None,
            modified: None,
            text: false,
        }
    }

    #[test]
    fn auto_asks_only_about_what_it_doesnt_know() {
        let mut items = vec![item("a.sh"), item("b.zip"), item("c.weird"), item("d.weird"), item("e.other")];
        let mut asked = Vec::new();
        let types = HashMap::from([("other".to_string(), true)]);
        let result = decide(&mut items, Mode::Auto, types, |name, ext| {
            asked.push((name.to_string(), ext.to_string()));
            Some(Answer { text: true, always: true, all: false })
        });
        assert!(result.is_ok());
        let texts: Vec<bool> = items.iter().map(|i| i.text).collect();
        if LINES_DIFFER {
            assert_eq!(texts, [true, false, true, true, true]);
            assert_eq!(asked, [("c.weird".to_string(), "weird".to_string())], "d.weird: the answer for always");
        } else {
            assert_eq!(texts, [false; 5], "the same line ends: copied as they are");
            assert!(asked.is_empty());
        }
    }

    #[test]
    fn cancel_stops_and_modes_override() {
        let mut items = vec![item("x.unknown")];
        let cancelled = decide(&mut items, Mode::Auto, HashMap::new(), |_, _| None);
        assert_eq!(cancelled.is_err(), LINES_DIFFER);
        decide(&mut items, Mode::Text, HashMap::new(), |_, _| unreachable!()).unwrap();
        assert_eq!(items[0].text, LINES_DIFFER);
        decide(&mut items, Mode::Binary, HashMap::new(), |_, _| unreachable!()).unwrap();
        assert!(!items[0].text);
        assert_eq!(extension(".gitignore"), "gitignore");
        assert_eq!(extension("Makefile"), "");
        assert_eq!(extension("A.TXT"), "txt");
    }
}
