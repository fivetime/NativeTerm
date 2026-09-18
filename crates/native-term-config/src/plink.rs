//! Non-SSH sessions (Telnet, rlogin, raw TCP, serial, SUPDUP), run by the
//! shim with PuTTY's console client `plink.exe`.
//!
//! `~/.ssh/config` only describes SSH hosts, so these live beside each
//! folder file: `lab.conf` → `lab.nt.toml` (the main `config` →
//! `config.nt.toml`). `Include config.d/*.conf` doesn't match that name,
//! so ssh never reads it, while it syncs, renames and backs up with its
//! folder. In the session tree each one is a [`HostEntry`] whose `plink`
//! is set; its `name` is its alias and shares the namespace with ssh's.
//!
//! ```toml
//! [[session]]
//! name = "core-switch"
//! label = "核心交换机"
//! protocol = "telnet"
//! host = "10.0.0.1"
//! charset = "gbk"
//!
//! [[session]]
//! name = "console-com3"
//! protocol = "serial"
//! serial = { line = "COM3", speed = 115200 }
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::tree::{HostEntry, NtKeys};

/// The second extension of a folder's non-SSH sessions file.
pub const EXTENSION: &str = "nt.toml";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    #[default]
    Telnet,
    Rlogin,
    Raw,
    Serial,
    Supdup,
}

impl Protocol {
    pub const ALL: [Protocol; 5] = [Protocol::Telnet, Protocol::Rlogin, Protocol::Raw, Protocol::Serial, Protocol::Supdup];

    /// As written in the file and on plink's command line (without `-`).
    pub fn name(self) -> &'static str {
        match self {
            Protocol::Telnet => "telnet",
            Protocol::Rlogin => "rlogin",
            Protocol::Raw => "raw",
            Protocol::Serial => "serial",
            Protocol::Supdup => "supdup",
        }
    }

    /// What plink connects to without `-P`.
    pub fn default_port(self) -> Option<u16> {
        match self {
            Protocol::Telnet => Some(23),
            Protocol::Rlogin => Some(513),
            Protocol::Supdup => Some(95),
            Protocol::Raw | Protocol::Serial => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Parity {
    #[default]
    None,
    Odd,
    Even,
    Mark,
    Space,
}

impl Parity {
    pub const ALL: [Parity; 5] = [Parity::None, Parity::Odd, Parity::Even, Parity::Mark, Parity::Space];

    fn letter(self) -> char {
        match self {
            Parity::None => 'n',
            Parity::Odd => 'o',
            Parity::Even => 'e',
            Parity::Mark => 'm',
            Parity::Space => 's',
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Flow {
    None,
    #[default]
    XonXoff,
    RtsCts,
    DsrDtr,
}

impl Flow {
    pub const ALL: [Flow; 4] = [Flow::None, Flow::XonXoff, Flow::RtsCts, Flow::DsrDtr];

    fn letter(self) -> char {
        match self {
            Flow::None => 'N',
            Flow::XonXoff => 'X',
            Flow::RtsCts => 'R',
            Flow::DsrDtr => 'D',
        }
    }
}

fn default_speed() -> u32 {
    9600
}

fn default_data_bits() -> u8 {
    8
}

fn default_stop_bits() -> String {
    "1".into()
}

/// PuTTY's Serial page (its defaults: 9600 8N1, XON/XOFF).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Serial {
    /// `COM3`
    pub line: String,
    #[serde(default = "default_speed")]
    pub speed: u32,
    #[serde(default = "default_data_bits")]
    pub data_bits: u8,
    #[serde(default)]
    pub parity: Parity,
    /// `1`, `1.5` or `2`
    #[serde(default = "default_stop_bits")]
    pub stop_bits: String,
    #[serde(default)]
    pub flow: Flow,
}

impl Serial {
    pub fn new(line: impl Into<String>) -> Serial {
        Serial {
            line: line.into(),
            speed: default_speed(),
            data_bits: default_data_bits(),
            parity: Parity::default(),
            stop_bits: default_stop_bits(),
            flow: Flow::default(),
        }
    }

    /// plink's `-sercfg`: `115200,8,n,1,N`.
    pub fn sercfg(&self) -> String {
        format!("{},{},{},{},{}", self.speed, self.data_bits, self.parity.letter(), self.stop_bits, self.flow.letter())
    }
}

/// A value for PuTTY's saved-session format (`REG_DWORD` or `REG_SZ`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PuttyValue {
    Number(u32),
    Text(String),
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlinkSession {
    /// The alias: unique among all sessions, ssh's included.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default)]
    pub protocol: Protocol,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// Auto-login user name (Telnet, rlogin).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// `utf-8` (default), `gbk`, … or a Windows code page number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charset: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<Serial>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub favorite: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Typed once the connection is up (there is no login signal).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_login: Option<String>,
    /// Where it came from (`putty:<name>`, `securecrt:<path>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// PuTTY options that plink only takes from a saved session; the shim
    /// writes them to a temporary one (`-load`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub putty: BTreeMap<String, PuttyValue>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SessionsFile {
    #[serde(default, rename = "session", skip_serializing_if = "Vec::is_empty")]
    sessions: Vec<PlinkSession>,
}

/// A PuTTY option plink only takes from a saved session, with PuTTY's own
/// default (a session doesn't store an option set to it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PuttyOption {
    Text { key: &'static str, default: &'static str },
    Number { key: &'static str, default: u32 },
    Flag { key: &'static str, default: bool },
}

impl PuttyOption {
    pub fn key(self) -> &'static str {
        match self {
            PuttyOption::Text { key, .. } | PuttyOption::Number { key, .. } | PuttyOption::Flag { key, .. } => key,
        }
    }

    pub fn default_value(self) -> PuttyValue {
        match self {
            PuttyOption::Text { default, .. } => PuttyValue::Text(default.into()),
            PuttyOption::Number { default, .. } => PuttyValue::Number(default),
            PuttyOption::Flag { default, .. } => PuttyValue::Number(u32::from(default)),
        }
    }
}

/// Connection options for every protocol.
pub const PUTTY_CONNECTION: [PuttyOption; 4] = [
    PuttyOption::Text { key: "TerminalType", default: "xterm" },
    PuttyOption::Number { key: "PingIntervalSecs", default: 0 },
    PuttyOption::Flag { key: "TCPNoDelay", default: true },
    PuttyOption::Flag { key: "TCPKeepalives", default: false },
];

/// PuTTY's Telnet page (as far as plink uses it).
pub const PUTTY_TELNET: [PuttyOption; 3] = [
    PuttyOption::Flag { key: "PassiveTelnet", default: false },
    PuttyOption::Flag { key: "TelnetKey", default: false },
    PuttyOption::Flag { key: "RFCEnviron", default: false },
];

/// PuTTY's SUPDUP page.
pub const PUTTY_SUPDUP: [PuttyOption; 4] = [
    PuttyOption::Text { key: "SUPDUPLocation", default: "The Internet" },
    PuttyOption::Number { key: "SUPDUPCharset", default: 0 },
    PuttyOption::Flag { key: "SUPDUPMoreProcessing", default: false },
    PuttyOption::Flag { key: "SUPDUPScrolling", default: false },
];

impl PlinkSession {
    /// An option's value: the session's, or PuTTY's default.
    pub fn putty_value(&self, option: PuttyOption) -> PuttyValue {
        self.putty.get(option.key()).cloned().unwrap_or_else(|| option.default_value())
    }

    /// Set an option; its default removes it (nothing to pass on).
    pub fn set_putty_value(&mut self, option: PuttyOption, value: PuttyValue) {
        if value == option.default_value() {
            self.putty.remove(option.key());
        } else {
            self.putty.insert(option.key().to_string(), value);
        }
    }
}

/// The non-SSH sessions file beside a folder file.
pub fn sibling(folder_file: &Path) -> PathBuf {
    let name = folder_file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let stem = name.strip_suffix(".conf").unwrap_or(&name);
    folder_file.with_file_name(format!("{stem}.{EXTENSION}"))
}

/// The sessions in `path`; none if the file doesn't exist.
pub fn read(path: &Path) -> Result<Vec<PlinkSession>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.to_string()),
    };
    parse(&text)
}

pub fn parse(text: &str) -> Result<Vec<PlinkSession>, String> {
    let file: SessionsFile = toml::from_str(text).map_err(|e| e.to_string())?;
    for s in &file.sessions {
        s.check()?;
    }
    Ok(file.sessions)
}

pub fn render(sessions: &[PlinkSession]) -> String {
    let body = toml::to_string(&SessionsFile { sessions: sessions.to_vec() }).unwrap_or_default();
    format!("# NativeTerm: non-SSH sessions of this folder (run with plink). ssh doesn't read this file.\n\n{body}")
}

/// A Windows code page for a charset name or number.
pub fn code_page(charset: Option<&str>) -> Result<u32, String> {
    let Some(charset) = charset.map(str::trim).filter(|c| !c.is_empty()) else { return Ok(65001) };
    if let Ok(n) = charset.parse::<u32>() {
        return Ok(n);
    }
    let known = match charset.to_ascii_lowercase().replace(['_', ' '], "-").as_str() {
        "utf-8" | "utf8" => 65001,
        "gbk" | "gb2312" | "cp936" | "gb18030" => 936,
        "big5" | "cp950" => 950,
        "shift-jis" | "sjis" | "cp932" => 932,
        "euc-kr" | "cp949" => 949,
        "iso-8859-1" | "latin1" | "latin-1" => 28591,
        "windows-1252" | "cp1252" => 1252,
        "koi8-r" => 20866,
        "cp437" | "ibm437" => 437,
        _ => return Err(format!("unknown charset {charset:?}")),
    };
    Ok(known)
}

/// A charset NativeTerm knows, from another program's name for it
/// (SecureCRT's "Output Transformer Name", PuTTY's `LineCodePage`:
/// `GBK`, `CP936`, `ISO-8859-1:1998 (Latin-1, West Europe)`, …). `None`:
/// UTF-8 (nothing to set), or not recognized.
pub fn charset_from(name: &str) -> Option<Result<String, String>> {
    let lower = name.trim().to_ascii_lowercase();
    if lower.is_empty() || lower == "default" || lower.replace('-', "").contains("utf8") {
        return None;
    }
    let known = if lower.contains("gb") || lower.contains("936") {
        "gbk"
    } else if lower.contains("big5") || lower.contains("950") {
        "big5"
    } else if lower.contains("shift") || lower.contains("sjis") || lower.contains("932") {
        "shift_jis"
    } else if lower.contains("euc-kr") || lower.contains("949") || lower.contains("korean") {
        "euc-kr"
    } else if lower.contains("8859-1") || lower.contains("latin-1") || lower.contains("latin1") {
        "iso-8859-1"
    } else if lower.contains("1252") {
        "windows-1252"
    } else if code_page(Some(&lower)).is_ok() {
        return Some(Ok(lower));
    } else {
        return Some(Err(name.trim().to_string()));
    };
    Some(Ok(known.to_string()))
}

fn plain(s: &str) -> bool {
    !s.is_empty() && !s.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, '"' | '\'' | ';'))
}

impl PlinkSession {
    /// What must hold before it is written or run.
    pub fn check(&self) -> Result<(), String> {
        if !plain(&self.name) || self.name.starts_with('-') {
            return Err(format!("{:?}: the name must be one word", self.name));
        }
        code_page(self.charset.as_deref())?;
        match self.protocol {
            Protocol::Serial => {
                let line = self.serial.as_ref().map(|s| s.line.as_str()).unwrap_or_default();
                if !plain(line) || line.starts_with('-') {
                    return Err(format!("{}: a serial session needs a line such as COM3", self.name));
                }
            }
            _ => {
                let host = self.host.as_deref().unwrap_or_default();
                if !plain(host) || host.starts_with('-') {
                    return Err(format!("{}: a host is needed", self.name));
                }
                if self.protocol == Protocol::Raw && self.port.is_none() {
                    return Err(format!("{}: a raw connection needs a port", self.name));
                }
            }
        }
        if let Some(user) = &self.user {
            if !plain(user) || user.starts_with('-') {
                return Err(format!("{}: invalid user {user:?}", self.name));
            }
        }
        Ok(())
    }

    pub fn label(&self) -> &str {
        self.label.as_deref().filter(|l| !l.is_empty()).unwrap_or(&self.name)
    }

    /// plink's arguments; `load`: a saved session with the `putty` options.
    pub fn arguments(&self, load: Option<&str>) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(load) = load {
            args.extend(["-load".to_string(), load.to_string()]);
        }
        match (self.protocol, &self.serial) {
            (Protocol::Serial, Some(serial)) => {
                args.extend(["-serial".to_string(), serial.line.clone(), "-sercfg".to_string(), serial.sercfg()]);
            }
            _ => {
                args.push(format!("-{}", self.protocol.name()));
                if let Some(port) = self.port {
                    args.extend(["-P".to_string(), port.to_string()]);
                }
                if let Some(user) = &self.user {
                    args.extend(["-l".to_string(), user.clone()]);
                }
                args.push(self.host.clone().unwrap_or_default());
            }
        }
        args
    }

    /// Where it connects, for the tree: `host[:port]` or the serial line.
    pub fn target(&self) -> String {
        match (self.protocol, &self.serial) {
            (Protocol::Serial, Some(s)) => s.line.clone(),
            _ => {
                let host = self.host.clone().unwrap_or_default();
                match self.port {
                    Some(port) if Some(port) != self.protocol.default_port() => format!("{host}:{port}"),
                    _ => host,
                }
            }
        }
    }

    /// The session as a tree entry of `file` (the `.nt.toml`).
    pub fn to_entry(&self, file: &Path) -> HostEntry {
        let mut nt = BTreeMap::new();
        let mut put = |k: &str, v: Option<&String>| {
            if let Some(v) = v.filter(|v| !v.is_empty()) {
                nt.insert(k.to_string(), v.clone());
            }
        };
        put("label", self.label.as_ref());
        put("id", self.id.as_ref());
        put("note", self.note.as_ref());
        put("onlogin", self.on_login.as_ref());
        put("source", self.source.as_ref());
        if self.favorite {
            nt.insert("favorite".into(), "yes".into());
        }
        let hostname = match self.protocol {
            Protocol::Serial => self.serial.as_ref().map(|s| s.line.clone()),
            _ => self.host.clone(),
        };
        HostEntry {
            aliases: vec![self.name.clone()],
            hostname,
            user: self.user.clone(),
            port: self.port,
            proxy_jump: None,
            identity_files: Vec::new(),
            nt: NtKeys(nt),
            file: file.to_path_buf(),
            line: 0,
            plink: Some(Box::new(self.clone())),
        }
    }
}

/// The non-SSH session `alias` in the tree of `ssh_dir`, if it is one.
pub fn find(ssh_dir: &Path, alias: &str) -> Option<PlinkSession> {
    let tree = crate::SessionTree::load(ssh_dir);
    let (_, host) = tree.find(alias)?;
    host.plink.as_deref().cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn telnet(name: &str, host: &str) -> PlinkSession {
        PlinkSession { name: name.into(), host: Some(host.into()), ..Default::default() }
    }

    #[test]
    fn file_round_trip() {
        let text = r#"
[[session]]
name = "core-switch"
label = "核心交换机"
protocol = "telnet"
host = "10.0.0.1"
charset = "gbk"
favorite = true

[session.putty]
PassiveTelnet = 1
TermType = "vt100"

[[session]]
name = "console-com3"
protocol = "serial"
serial = { line = "COM3", speed = 115200 }
"#;
        let sessions = parse(text).unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].label(), "核心交换机");
        assert_eq!(sessions[0].putty["PassiveTelnet"], PuttyValue::Number(1));
        assert_eq!(sessions[0].putty["TermType"], PuttyValue::Text("vt100".into()));
        let serial = sessions[1].serial.as_ref().unwrap();
        assert_eq!(serial.sercfg(), "115200,8,n,1,X", "PuTTY's defaults for the rest");
        assert_eq!(parse(&render(&sessions)).unwrap(), sessions);
        assert!(parse("").unwrap().is_empty());
    }

    #[test]
    fn plink_arguments() {
        assert_eq!(telnet("sw", "10.0.0.1").arguments(None), ["-telnet", "10.0.0.1"]);
        let mut s = telnet("sw", "10.0.0.1");
        s.protocol = Protocol::Raw;
        s.port = Some(4001);
        s.user = Some("admin".into());
        assert_eq!(s.arguments(Some("NativeTerm-x")), ["-load", "NativeTerm-x", "-raw", "-P", "4001", "-l", "admin", "10.0.0.1"]);
        let serial = PlinkSession {
            name: "c".into(),
            protocol: Protocol::Serial,
            serial: Some(Serial { flow: Flow::None, ..Serial::new("COM3") }),
            ..Default::default()
        };
        assert_eq!(serial.arguments(None), ["-serial", "COM3", "-sercfg", "9600,8,n,1,N"]);
        assert_eq!(serial.target(), "COM3");
        assert_eq!(s.target(), "10.0.0.1:4001");
        assert_eq!(telnet("sw", "h").target(), "h");
    }

    #[test]
    fn checks() {
        assert!(telnet("sw", "10.0.0.1").check().is_ok());
        assert!(telnet("two words", "h").check().is_err());
        assert!(telnet("sw", "-oProxyCommand=x").check().is_err(), "no options through the host");
        assert!(telnet("sw", "").check().is_err());
        let mut raw = telnet("r", "h");
        raw.protocol = Protocol::Raw;
        assert!(raw.check().is_err(), "raw needs a port");
        let mut serial = telnet("c", "");
        serial.protocol = Protocol::Serial;
        assert!(serial.check().is_err(), "serial needs a line");
        serial.serial = Some(Serial::new("COM7"));
        assert!(serial.check().is_ok());
        let mut gbk = telnet("g", "h");
        gbk.charset = Some("GBK".into());
        assert!(gbk.check().is_ok());
        gbk.charset = Some("klingon".into());
        assert!(gbk.check().is_err());
    }

    #[test]
    fn putty_options_keep_only_changes() {
        let mut s = telnet("sw", "h");
        let passive = PUTTY_TELNET[0];
        assert_eq!(s.putty_value(passive), PuttyValue::Number(0));
        s.set_putty_value(passive, PuttyValue::Number(1));
        s.set_putty_value(PUTTY_CONNECTION[0], PuttyValue::Text("xterm".into()));
        assert_eq!(s.putty.len(), 1, "a default isn't stored: {:?}", s.putty);
        s.putty.insert("SomethingElse".into(), PuttyValue::Number(7));
        s.set_putty_value(passive, PuttyValue::Number(0));
        assert_eq!(s.putty.keys().collect::<Vec<_>>(), ["SomethingElse"], "unknown keys stay");
    }

    #[test]
    fn charsets_of_other_programs() {
        assert_eq!(charset_from("GBK"), Some(Ok("gbk".into())));
        assert_eq!(charset_from("CP936"), Some(Ok("gbk".into())));
        assert_eq!(charset_from("ISO-8859-1:1998 (Latin-1, West Europe)"), Some(Ok("iso-8859-1".into())));
        assert_eq!(charset_from("UTF-8"), None);
        assert_eq!(charset_from("Default"), None);
        assert_eq!(charset_from("KOI8-R"), Some(Ok("koi8-r".into())));
        assert_eq!(charset_from("Klingon"), Some(Err("Klingon".into())));
    }

    #[test]
    fn code_pages() {
        assert_eq!(code_page(None), Ok(65001));
        assert_eq!(code_page(Some("gbk")), Ok(936));
        assert_eq!(code_page(Some("Shift_JIS")), Ok(932));
        assert_eq!(code_page(Some("950")), Ok(950));
    }

    #[test]
    fn sibling_names() {
        assert_eq!(sibling(Path::new(r"C:\h\.ssh\config.d\lab.conf")), Path::new(r"C:\h\.ssh\config.d\lab.nt.toml"));
        assert_eq!(sibling(Path::new(r"C:\h\.ssh\config")), Path::new(r"C:\h\.ssh\config.nt.toml"));
    }

    /// Loaded into the tree beside the folder's ssh hosts.
    #[test]
    fn in_the_tree() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().join("config.d");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(dir.path().join("config"), "Include config.d/*.conf\n").unwrap();
        std::fs::write(d.join("lab.conf"), "Host web\n    HostName 10.0.0.9\n").unwrap();
        std::fs::write(d.join("lab.nt.toml"), render(&[telnet("sw", "10.0.0.1")])).unwrap();
        let tree = SessionTree::load_with(dir.path(), dir.path());
        let lab = tree.folders.iter().find(|f| f.name == "lab").unwrap();
        let names: Vec<&str> = lab.hosts.iter().map(|h| h.alias()).collect();
        assert_eq!(names, ["web", "sw"]);
        assert_eq!(lab.hosts[1].target(), "10.0.0.1");
        assert!(lab.hosts[0].plink.is_none() && lab.hosts[1].plink.is_some());
        assert_eq!(find(dir.path(), "sw").unwrap().host.as_deref(), Some("10.0.0.1"));
        assert!(find(dir.path(), "web").is_none(), "an ssh host");

        // a broken file is reported, the ssh hosts still load
        std::fs::write(d.join("lab.nt.toml"), "[[session]]\nname = 'x y'\nhost = 'h'\n").unwrap();
        let tree = SessionTree::load_with(dir.path(), dir.path());
        assert_eq!(tree.hosts().count(), 1);
        assert!(tree.warnings.iter().any(|w| matches!(w, crate::Warning::Unreadable { .. })));
    }

    use crate::SessionTree;
}
