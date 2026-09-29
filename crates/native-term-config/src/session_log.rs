//! The session log: what a session's terminal was sent, written to a file
//! on this computer, with SecureCRT's options (Session Options → Terminal
//! → Log File). NativeTerm's shim writes it for every kind of session: an
//! SSH session's output comes from NativeTerm's ssh, a Telnet or serial
//! one's from ntplink (see `docs/SESSION-LOG.md`).
//!
//! The settings are `NativeTermLog*` keys: in a host's block, or in the
//! folder's `Host __nativeterm_folder__` block as the default for its
//! sessions; a non-SSH session keeps them in its `log` table. A session
//! with any log key of its own uses its own set, whole; one without uses
//! its folder's. Nothing is logged unless a log is started: on connecting
//! (`NativeTermLogStart yes`) or from the session's menu.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::tree::{Folder, HostEntry};

/// SecureCRT's log options, as NativeTerm keeps them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LogSettings {
    /// The file, with substitutions (`%H`, `%Y`, …); empty: the default,
    /// `logs` in NativeTerm's data folder (see `default_file`).
    pub file: String,
    /// Ask for the file each time a log starts.
    pub prompt: bool,
    /// Start logging when the session connects.
    pub start: bool,
    /// Every byte the terminal was sent, escape sequences included; else
    /// the text, a line at a time.
    pub raw: bool,
    /// A new file after midnight (only where the name has `%D`).
    pub midnight: bool,
    /// Add to an existing file; else a log that starts replaces it.
    pub append: bool,
    /// Each line starts with the time it began.
    pub timestamp: bool,
    /// How much of the client's own messages go in too: 0 none, 1 verbose,
    /// 2–4 debug levels (ssh's `LogLevel` VERBOSE, DEBUG1–3); ntplink's
    /// event log for any level above 0.
    pub trace: u8,
    /// Written when the session connects, disconnects, and before each
    /// line (with substitutions).
    pub upon_connect: String,
    pub upon_disconnect: String,
    pub each_line: String,
    /// Only the three texts above; the session's own output is left out.
    pub only_custom: bool,
}

/// The highest trace level.
pub const MAX_TRACE: u8 = 9;

/// The keys, lowercase without the `NativeTerm` prefix, with the name
/// they are written with.
const KEYS: [(&str, &str); 12] = [
    ("logfile", "NativeTermLogFile"),
    ("logprompt", "NativeTermLogPrompt"),
    ("logstart", "NativeTermLogStart"),
    ("lograw", "NativeTermLogRaw"),
    ("logmidnight", "NativeTermLogMidnight"),
    ("logappend", "NativeTermLogAppend"),
    ("logtimestamp", "NativeTermLogTimestamp"),
    ("logtrace", "NativeTermLogTrace"),
    ("loguponconnect", "NativeTermLogUponConnect"),
    ("logupondisconnect", "NativeTermLogUponDisconnect"),
    ("logeachline", "NativeTermLogEachLine"),
    ("logonlycustom", "NativeTermLogOnlyCustom"),
];

/// Every log key as it is written (`NativeTermLogFile`, …).
pub fn directive_names() -> impl Iterator<Item = &'static str> {
    KEYS.iter().map(|(_, name)| *name)
}

/// Whether a lowercase `NativeTerm*` key (without the prefix) is a log key.
pub fn is_log_key(key: &str) -> bool {
    KEYS.iter().any(|(k, _)| k.eq_ignore_ascii_case(key))
}

fn yes(value: Option<&str>) -> bool {
    value.is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "yes" | "true" | "1" | "on"))
}

impl LogSettings {
    /// From keys read with `get` (lowercase, without the prefix); `None`
    /// when there is no log key at all.
    pub fn from_keys<'a>(get: impl Fn(&str) -> Option<&'a str>) -> Option<LogSettings> {
        if !KEYS.iter().any(|(k, _)| get(k).is_some()) {
            return None;
        }
        let text = |k: &str| get(k).map(str::to_string).unwrap_or_default();
        Some(LogSettings {
            file: text("logfile").trim().to_string(),
            prompt: yes(get("logprompt")),
            start: yes(get("logstart")),
            raw: yes(get("lograw")),
            midnight: yes(get("logmidnight")),
            append: yes(get("logappend")),
            timestamp: yes(get("logtimestamp")),
            trace: get("logtrace").and_then(|v| v.trim().parse::<u8>().ok()).unwrap_or(0).min(MAX_TRACE),
            upon_connect: text("loguponconnect"),
            upon_disconnect: text("logupondisconnect"),
            each_line: text("logeachline"),
            only_custom: yes(get("logonlycustom")),
        })
    }

    /// The keys to write, as (written name, value); every key is written,
    /// so that a session's own set stays whole (and its folder's doesn't
    /// show through a key left out).
    pub fn directives(&self) -> Vec<(&'static str, String)> {
        let flag = |on: bool| if on { "yes" } else { "no" }.to_string();
        let values = [
            self.file.trim().to_string(),
            flag(self.prompt),
            flag(self.start),
            flag(self.raw),
            flag(self.midnight),
            flag(self.append),
            flag(self.timestamp),
            self.trace.min(MAX_TRACE).to_string(),
            self.upon_connect.clone(),
            self.upon_disconnect.clone(),
            self.each_line.clone(),
            flag(self.only_custom),
        ];
        KEYS.iter().zip(values).map(|((_, name), value)| (*name, value)).collect()
    }

    /// The same as lowercase keys (a non-SSH session's `log` table).
    pub fn to_keys(&self) -> BTreeMap<String, String> {
        KEYS.iter().zip(self.directives()).map(|((key, _), (_, value))| (key.to_string(), value)).collect()
    }

    /// The session's own set, if it has one.
    pub fn own(host: &HostEntry) -> Option<LogSettings> {
        LogSettings::from_keys(|k| host.nt.get(k)).or_else(|| host.plink.as_ref().and_then(|p| p.legacy_log()))
    }

    /// The folder's default set, if it has one.
    pub fn of_folder(folder: &Folder) -> Option<LogSettings> {
        LogSettings::from_keys(|k| folder.defaults.get(k))
    }

    /// What a session uses: its own set, else its folder's, else nothing
    /// set (no log starts by itself).
    pub fn for_host(folder: &Folder, host: &HostEntry) -> LogSettings {
        LogSettings::own(host).or_else(|| LogSettings::of_folder(folder)).unwrap_or_default()
    }

    /// Whether the custom texts say anything ("Log only custom data" is
    /// offered then).
    pub fn has_custom(&self) -> bool {
        [&self.upon_connect, &self.upon_disconnect, &self.each_line].iter().any(|t| !t.trim().is_empty())
    }
}

/// Where a log goes when no file is named: `logs` in NativeTerm's data
/// folder, a file per host and day.
pub const DEFAULT_NAME: &str = "%H-%Y%M%D.log";

/// The default file name for `data_dir`.
pub fn default_file(data_dir: &Path) -> PathBuf {
    data_dir.join("logs").join(DEFAULT_NAME)
}

/// A moment, as the substitutions show it (local time).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stamp {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub milli: u32,
}

/// What `%H`, `%S`, `%P` and `%F` stand for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Names {
    /// The host name or address (a serial session: its line, `COM3`).
    pub host: String,
    /// The session's name as the tree shows it.
    pub session: String,
    pub port: Option<u16>,
    /// The folder's name.
    pub folder: String,
}

/// Replace SecureCRT's substitutions in `text`: `%H` host, `%S` session,
/// `%P` port, `%F` folder, `%Y` `%y` year, `%M` month, `%D` day, `%h` hour,
/// `%m` minute, `%s` seconds, `%t` milliseconds, `%%` a percent sign, and
/// `%NAME%` an environment variable (asked with `env`). Anything else is
/// left as it is. `for_file`: the names (host, session, folder) are made
/// safe in a file name; a variable is taken as it is (`%USERPROFILE%` is
/// a folder).
pub fn expand(text: &str, names: &Names, at: &Stamp, env: impl Fn(&str) -> Option<String>, for_file: bool) -> String {
    let safe = |s: &str| if for_file { file_safe(s) } else { s.to_string() };
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '%' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        // %NAME%: an environment variable that is set
        if let Some(end) = chars[i + 1..].iter().position(|c| *c == '%').map(|n| i + 1 + n) {
            let name: String = chars[i + 1..end].iter().collect();
            let valid = name.len() >= 2
                && name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            if valid {
                if let Some(value) = env(&name) {
                    out.push_str(&value);
                    i = end + 1;
                    continue;
                }
            }
        }
        let Some(&code) = chars.get(i + 1) else {
            out.push('%');
            break;
        };
        let replaced = match code {
            '%' => Some("%".to_string()),
            'H' => Some(safe(&names.host)),
            'S' => Some(safe(&names.session)),
            'P' => Some(names.port.map(|p| p.to_string()).unwrap_or_default()),
            'F' => Some(safe(&names.folder)),
            'Y' => Some(format!("{:04}", at.year)),
            'y' => Some(format!("{:02}", at.year.rem_euclid(100))),
            'M' => Some(format!("{:02}", at.month)),
            'D' => Some(format!("{:02}", at.day)),
            'h' => Some(format!("{:02}", at.hour)),
            'm' => Some(format!("{:02}", at.minute)),
            's' => Some(format!("{:02}", at.second)),
            't' => Some(format!("{:03}", at.milli)),
            _ => None,
        };
        match replaced {
            Some(text) => {
                out.push_str(&text);
                i += 2;
            }
            None => {
                out.push('%');
                i += 1;
            }
        }
    }
    out
}

/// Characters no file name may hold (on any system NativeTerm runs on)
/// become `_`.
pub fn file_safe(name: &str) -> String {
    name.chars()
        .map(
            |c| {
                if matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() {
                    '_'
                } else {
                    c
                }
            },
        )
        .collect()
}

/// Whether a new file after midnight can differ from today's (the name
/// has the day in it).
pub fn names_the_day(template: &str) -> bool {
    template.contains("%D")
}

/// The log file for a template: the default one when empty; a relative
/// name goes into `logs` in the data folder; `~` is the home folder.
pub fn resolve(template: &str, data_dir: &Path, home: Option<&Path>) -> PathBuf {
    let template = template.trim();
    if template.is_empty() {
        return default_file(data_dir);
    }
    if let (Some(rest), Some(home)) = (template.strip_prefix("~/").or_else(|| template.strip_prefix("~\\")), home) {
        return home.join(rest);
    }
    let path = Path::new(template);
    if path.is_absolute() || template.starts_with('\\') || template.starts_with('/') {
        path.to_path_buf()
    } else {
        data_dir.join("logs").join(path)
    }
}

/// PuTTY's log file name codes (`&H` `&Y` `&M` `&D` `&T` `&P`, the ones
/// ntplink sessions were given before) in SecureCRT's form.
pub fn from_putty_name(name: &str) -> String {
    name.replace("&H", "%H")
        .replace("&Y", "%Y")
        .replace("&M", "%M")
        .replace("&D", "%D")
        .replace("&T", "%h%m%s")
        .replace("&P", "%P")
        .replace("&&", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at() -> Stamp {
        Stamp { year: 2026, month: 9, day: 3, hour: 7, minute: 5, second: 9, milli: 42 }
    }

    fn names() -> Names {
        Names { host: "10.0.0.1".into(), session: "core/sw1".into(), port: Some(22), folder: "Lab".into() }
    }

    #[test]
    fn substitutions_as_securecrt_has_them() {
        let env = |name: &str| (name == "USERNAME").then(|| "simon".to_string());
        let out = expand("%S_%H_%P_%Y%M%D-%h%m%s.%t_%y_%F_%%_%USERNAME%_%NOPE%_%Q", &names(), &at(), env, false);
        assert_eq!(out, "core/sw1_10.0.0.1_22_20260903-070509.042_26_Lab_%_simon_%NOPE%_%Q");
        // in a file name, the names can't make folders
        assert_eq!(expand("%S-%Y%M%D.log", &names(), &at(), |_| None, true), "core_sw1-20260903.log");
        // a lone % at the end stays
        assert_eq!(expand("50%", &names(), &at(), |_| None, false), "50%");
    }

    #[test]
    fn keys_round_trip_and_a_session_keeps_its_set_whole() {
        let settings = LogSettings {
            file: "%S.log".into(),
            start: true,
            append: true,
            timestamp: true,
            trace: 3,
            upon_connect: "== %S connected ==".into(),
            ..LogSettings::default()
        };
        let keys = settings.to_keys();
        let read = LogSettings::from_keys(|k| keys.get(k).map(String::as_str)).unwrap();
        assert_eq!(read, settings);
        assert_eq!(settings.directives().len(), 12);
        assert!(LogSettings::from_keys(|_| None).is_none());
        // a level above the highest is capped
        let keys: BTreeMap<String, String> = [("logtrace".to_string(), "40".to_string())].into();
        assert_eq!(LogSettings::from_keys(|k| keys.get(k).map(String::as_str)).unwrap().trace, MAX_TRACE);
    }

    #[test]
    fn a_host_uses_its_own_set_else_its_folders() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("lab.conf"),
            concat!(
                "Host __nativeterm_folder__\n    NativeTermLogStart yes\n    NativeTermLogFile \"%S %Y.log\"\n\n",
                "Host sw1\n    HostName 10.0.0.1\n\n",
                "Host sw2\n    HostName 10.0.0.2\n    NativeTermLogRaw yes\n",
            ),
        )
        .unwrap();
        std::fs::write(dir.path().join("config"), "Include lab.conf\n").unwrap();
        let tree = crate::tree::SessionTree::load(dir.path());
        let (folder, sw1) = tree.find("sw1").unwrap();
        let own = LogSettings::for_host(folder, sw1);
        assert!(own.start);
        assert_eq!(own.file, "%S %Y.log");
        let (folder, sw2) = tree.find("sw2").unwrap();
        let own = LogSettings::for_host(folder, sw2);
        // its own set, whole: the folder's start and file don't come through
        assert!(own.raw && !own.start && own.file.is_empty());
    }

    #[test]
    fn files_go_into_the_logs_folder_unless_named_fully() {
        let data = Path::new("/data");
        assert_eq!(resolve("", data, None), data.join("logs").join(DEFAULT_NAME));
        assert_eq!(resolve("a/b.log", data, None), data.join("logs").join("a/b.log"));
        assert_eq!(resolve("~/x.log", data, Some(Path::new("/home/u"))), Path::new("/home/u").join("x.log"));
        assert!(resolve("/var/log/x.log", data, None).ends_with("x.log"));
    }

    #[test]
    fn putty_names_become_securecrt_ones() {
        assert_eq!(from_putty_name("C:\\logs\\&H-&Y&M&D-&T.log"), "C:\\logs\\%H-%Y%M%D-%h%m%s.log");
        assert!(names_the_day("%H-%Y%M%D.log"));
        assert!(!names_the_day("%H.log"));
    }
}
