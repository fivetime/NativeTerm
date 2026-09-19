//! Import from SecureCRT (see `docs/ARCHITECTURE.md`, "Import from
//! SecureCRT").
//!
//! SecureCRT keeps one `.ini` file per session under `<Config
//! Path>\Sessions`, in folders. Lines look like
//!
//! ```text
//! S:"Hostname"=10.0.0.5          string
//! D:"[SSH2] Port"=00000016       32-bit hex number
//! Z:"Description"=00000002       string list: the next 2 lines
//!  first line
//!  second line
//! B:"Window Placement"=0000002c  binary: hex lines follow
//!  2c 00 00 00 ...
//! ```
//!
//! Secrets are never read into the model: values of keys that look like
//! passwords, and logon scripts (which often contain them), are skipped;
//! only whether they exist is recorded, for the report.
//!
//! [`scan`] reads the files, [`plan`] decides aliases, folders and what is
//! skipped, and [`crate::ops::Editor::import`] writes the plan.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::alias;
use crate::tree::SessionTree;

/// `NativeTermSource` value prefix: where an imported host came from, so a
/// second import skips it.
pub const SOURCE_PREFIX: &str = "securecrt:";

/// Which program the sessions come from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Origin {
    #[default]
    SecureCrt,
    /// PuTTY's saved sessions (see [`crate::putty`]).
    Putty,
}

impl Origin {
    /// `NativeTermSource` prefix.
    pub fn source_prefix(self) -> &'static str {
        match self {
            Origin::SecureCrt => SOURCE_PREFIX,
            Origin::Putty => "putty:",
        }
    }

    /// Folder for sessions that aren't in a folder.
    pub fn root_label(self) -> &'static str {
        match self {
            Origin::SecureCrt => ROOT_FOLDER_LABEL,
            Origin::Putty => "PuTTY",
        }
    }

    fn alias_prefix(self) -> &'static str {
        match self {
            Origin::SecureCrt => "securecrt",
            Origin::Putty => "putty",
        }
    }
}

/// A value from a session file.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Value {
    Str(String),
    Num(u32),
    List(Vec<String>),
}

#[derive(Debug, Default)]
struct Ini {
    values: HashMap<String, Value>,
    /// Keys whose values were skipped because they may hold secrets, with
    /// whether the value was non-empty.
    secrets: HashMap<String, bool>,
}

fn is_secret_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    k.contains("password") || k.contains("passphrase") || k.contains("login script") || k.contains("logon script")
}

fn parse_ini(text: &str) -> Ini {
    let mut ini = Ini::default();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let Some((kind, rest)) = line.split_once(":\"") else { continue };
        let Some((key, value)) = rest.split_once("\"=") else { continue };
        let secret = is_secret_key(key);
        match kind {
            "S" => {
                if secret {
                    ini.secrets.insert(key.to_string(), !value.is_empty());
                } else {
                    ini.values.insert(key.to_string(), Value::Str(value.to_string()));
                }
            }
            "D" => {
                if let Ok(n) = u32::from_str_radix(value.trim(), 16) {
                    if !secret {
                        ini.values.insert(key.to_string(), Value::Num(n));
                    }
                }
            }
            "Z" => {
                let count = usize::from_str_radix(value.trim(), 16).unwrap_or(0);
                let mut items = Vec::with_capacity(count.min(1024));
                for _ in 0..count {
                    let Some(item) = lines.next() else { break };
                    if !secret {
                        items.push(item.strip_prefix(' ').unwrap_or(item).to_string());
                    }
                }
                if secret {
                    ini.secrets.insert(key.to_string(), count > 0);
                } else {
                    ini.values.insert(key.to_string(), Value::List(items));
                }
            }
            "B" => {
                // hex lines, indented; nothing NativeTerm needs
                while lines.peek().is_some_and(|l| l.starts_with(' ')) {
                    lines.next();
                }
                if secret {
                    ini.secrets.insert(key.to_string(), true);
                }
            }
            _ => {}
        }
    }
    ini
}

impl Ini {
    fn str(&self, key: &str) -> Option<&str> {
        match self.values.get(key) {
            Some(Value::Str(s)) => Some(s.as_str()).filter(|s| !s.trim().is_empty()),
            _ => None,
        }
    }

    fn num(&self, key: &str) -> Option<u32> {
        match self.values.get(key) {
            Some(Value::Num(n)) => Some(*n),
            _ => None,
        }
    }

    fn list(&self, key: &str) -> &[String] {
        match self.values.get(key) {
            Some(Value::List(items)) => items,
            _ => &[],
        }
    }
}

/// How a session reaches its host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Firewall {
    None,
    /// Through another session (a jump host), by its path under `Sessions`.
    Session(String),
    /// A firewall/proxy defined in SecureCRT's global options, or a PuTTY
    /// proxy that isn't an SSH jump.
    Named(String),
    /// A jump host written out (`user@host:port`), not a saved session.
    Jump(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ForwardKind {
    Local,
    Remote,
    Dynamic,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Forward {
    pub kind: ForwardKind,
    pub name: String,
    pub bind: Option<String>,
    pub port: u16,
    /// `host:port`; `None` for dynamic forwards.
    pub target: Option<String>,
}

impl Forward {
    /// The ssh keyword and its value.
    pub fn directive(&self) -> (&'static str, String) {
        let listen = match &self.bind {
            Some(bind) if bind.contains(':') => format!("[{bind}]:{}", self.port),
            Some(bind) => format!("{bind}:{}", self.port),
            None => self.port.to_string(),
        };
        match self.kind {
            ForwardKind::Local => ("LocalForward", format!("{listen} {}", self.target.as_deref().unwrap_or(""))),
            ForwardKind::Remote => ("RemoteForward", format!("{listen} {}", self.target.as_deref().unwrap_or(""))),
            ForwardKind::Dynamic => ("DynamicForward", listen),
        }
    }
}

/// `Name|ListenHost,ListenPort|TargetHostDiff?|TargetHost|TargetPort||`
/// (from VanDyke's own import script). A target that isn't "different
/// from the SSH server" is the server itself, `localhost` from its side.
fn parse_forward(line: &str, reverse: bool) -> Option<Forward> {
    let fields: Vec<&str> = line.split('|').collect();
    if fields.len() < 5 {
        return None;
    }
    let (bind, port) = match fields[1].rsplit_once(',') {
        Some((bind, port)) => (Some(bind.trim()).filter(|b| !b.is_empty()), port),
        None => (None, fields[1]),
    };
    let port: u16 = port.trim().parse().ok().filter(|p| *p != 0)?;
    let different = fields[2].trim() == "1";
    let target_host = fields[3].trim();
    let (kind, target) = if !reverse && target_host.starts_with("socks") {
        (ForwardKind::Dynamic, None)
    } else {
        let target_port: u16 = fields[4].trim().parse().ok().filter(|p| *p != 0)?;
        let host = if different && !target_host.is_empty() { target_host } else { "localhost" };
        let host = if host.contains(':') { format!("[{host}]") } else { host.to_string() };
        (if reverse { ForwardKind::Remote } else { ForwardKind::Local }, Some(format!("{host}:{target_port}")))
    };
    Some(Forward { kind, name: fields[0].to_string(), bind: bind.map(str::to_string), port, target })
}

/// One session file, reduced to what NativeTerm uses. No secrets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrtSession {
    /// Folder names from `Sessions` down.
    pub folder: Vec<String>,
    /// The session's name (file name without `.ini`).
    pub name: String,
    /// `folder/sub/name`, as SecureCRT refers to sessions.
    pub path: String,
    pub protocol: String,
    pub hostname: Option<String>,
    pub port: Option<u16>,
    pub username: Option<String>,
    pub description: Vec<String>,
    pub firewall: Firewall,
    pub identity_file: Option<String>,
    pub forwards: Vec<Forward>,
    /// Forward lines that couldn't be read.
    pub bad_forwards: usize,
    /// "Output Transformer Name" when it isn't UTF-8/default.
    pub encoding: Option<String>,
    pub com_port: Option<String>,
    pub logon_actions: bool,
    pub saved_password: bool,
    /// A PuTTY-format key (`.ppk`), which OpenSSH can't use.
    pub ppk_key: Option<String>,
    /// More directives to write (`ForwardAgent yes`, …).
    pub options: Vec<(&'static str, String)>,
    /// A serial session's line settings.
    pub serial: Option<crate::plink::Serial>,
    /// PuTTY options for a non-SSH session (see `PlinkSession::putty`).
    pub putty: BTreeMap<String, crate::plink::PuttyValue>,
}

fn session_from(ini: &Ini, folder: Vec<String>, name: String) -> CrtSession {
    let protocol = ini.str("Protocol Name").unwrap_or("SSH2").to_string();
    let port_key = if protocol.eq_ignore_ascii_case("SSH2") { "[SSH2] Port" } else { "Port" };
    let port = ini.num(port_key).and_then(|p| u16::try_from(p).ok()).filter(|p| *p != 0);
    let firewall = match ini.str("Firewall Name").map(str::trim) {
        None => Firewall::None,
        Some(f) if f.eq_ignore_ascii_case("none") => Firewall::None,
        Some(f) => match f.strip_prefix("Session:") {
            Some(path) => Firewall::Session(normalize_path(path)),
            None => Firewall::Named(f.to_string()),
        },
    };
    // "Use Global Public Key" = 0 means the session names its own key
    let identity_file = match ini.num("Use Global Public Key") {
        Some(0) => ini.str("Identity Filename V2").map(identity_path),
        _ => None,
    };
    let mut forwards = Vec::new();
    let mut bad_forwards = 0;
    for (key, reverse) in [("Port Forward Table V2", false), ("Reverse Forward Table V2", true)] {
        for line in ini.list(key).iter().filter(|l| !l.trim().is_empty()) {
            match parse_forward(line, reverse) {
                Some(f) => forwards.push(f),
                None => bad_forwards += 1,
            }
        }
    }
    let encoding = ini
        .str("Output Transformer Name")
        .filter(|e| {
            !e.eq_ignore_ascii_case("default") && !e.eq_ignore_ascii_case("utf-8") && !e.eq_ignore_ascii_case("utf8")
        })
        .map(str::to_string);
    let logon_actions = ini.num("Use Login Script").is_some_and(|n| n != 0)
        || ini.secrets.iter().any(|(k, v)| *v && k.to_ascii_lowercase().contains("login script"));
    let saved_password = ini.secrets.iter().any(|(k, v)| *v && k.to_ascii_lowercase().starts_with("password"));
    let mut path: Vec<&str> = folder.iter().map(String::as_str).collect();
    path.push(&name);
    CrtSession {
        path: path.join("/"),
        folder,
        name,
        protocol,
        hostname: ini.str("Hostname").map(|h| h.trim().to_string()),
        port,
        username: ini.str("Username").map(|u| u.trim().to_string()),
        description: ini.list("Description").iter().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect(),
        firewall,
        identity_file,
        forwards,
        bad_forwards,
        encoding,
        com_port: ini.str("Com Port").map(str::to_string),
        logon_actions,
        saved_password,
        ppk_key: None,
        options: Vec::new(),
        serial: ini.str("Com Port").filter(|_| protocol_is(ini, "serial")).map(|line| serial_from(ini, line)),
        putty: BTreeMap::new(),
    }
}

fn protocol_is(ini: &Ini, name: &str) -> bool {
    ini.str("Protocol Name").is_some_and(|p| p.eq_ignore_ascii_case(name))
}

/// SecureCRT's serial page: numbers as Windows' `DCB` has them (parity
/// 0–4 none/odd/even/mark/space, stop bits 0/1/2 = 1/1.5/2); flow control
/// is one of `CTS Flow` (RTS/CTS), `DSR Flow` (DSR/DTR), `XON Flow`.
fn serial_from(ini: &Ini, line: &str) -> crate::plink::Serial {
    use crate::plink::{Flow, Parity, Serial};
    let mut serial = Serial::new(line.trim());
    if let Some(speed) = ini.num("Baud Rate").filter(|s| *s > 0) {
        serial.speed = speed;
    }
    if let Some(bits) = ini.num("Data Bits").filter(|b| (5..=8).contains(b)) {
        serial.data_bits = bits as u8;
    }
    serial.parity = match ini.num("Parity") {
        Some(1) => Parity::Odd,
        Some(2) => Parity::Even,
        Some(3) => Parity::Mark,
        Some(4) => Parity::Space,
        _ => Parity::None,
    };
    serial.stop_bits = match ini.num("Stop Bits") {
        Some(1) => "1.5",
        Some(2) => "2",
        _ => "1",
    }
    .to_string();
    let on = |key: &str| ini.num(key).is_some_and(|n| n != 0);
    serial.flow = if on("CTS Flow") {
        Flow::RtsCts
    } else if on("DSR Flow") {
        Flow::DsrDtr
    } else if on("XON Flow") {
        Flow::XonXoff
    } else {
        Flow::None
    };
    serial
}

fn normalize_path(path: &str) -> String {
    path.trim().replace('\\', "/").trim_matches('/').to_string()
}

/// SecureCRT stores the public key's path (sometimes with `::` options);
/// OpenSSH wants the private key next to it.
fn identity_path(value: &str) -> String {
    let path = value.split("::").next().unwrap_or(value).trim();
    path.strip_suffix(".pub").unwrap_or(path).to_string()
}

/// What a scan found.
#[derive(Debug, Default)]
pub struct Scan {
    pub origin: Origin,
    pub root: PathBuf,
    pub sessions: Vec<CrtSession>,
    /// Folders seen (paths), including empty ones.
    pub folders: Vec<String>,
    /// Files that couldn't be read, with the error.
    pub unreadable: Vec<(String, String)>,
    /// Files that weren't UTF-8 (read with replacement characters).
    pub not_utf8: Vec<String>,
    /// Host keys from SecureCRT's `KnownHosts` folder.
    pub host_keys: crate::known_hosts::KeyScan,
}

/// Read every session under `<config>\Sessions`. `config` is SecureCRT's
/// "Config Path" (the folder holding `Sessions`); the `Sessions` folder
/// itself is accepted too.
pub fn scan(config: &Path) -> io::Result<Scan> {
    let root = if config.join("Sessions").is_dir() { config.join("Sessions") } else { config.to_path_buf() };
    if !root.is_dir() {
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("{} has no Sessions folder", config.display())));
    }
    let mut out = Scan { root: root.clone(), ..Scan::default() };
    walk(&root, &mut Vec::new(), &mut out)?;
    // next to `Sessions`
    if let Some(config) = root.parent() {
        out.host_keys = crate::known_hosts::scan_securecrt(config).unwrap_or_default();
    }
    out.sessions.sort_by_key(|s| s.path.to_lowercase());
    out.folders.sort_by_key(|f| f.to_lowercase());
    Ok(out)
}

fn walk(dir: &Path, folder: &mut Vec<String>, out: &mut Scan) -> io::Result<()> {
    let mut entries: Vec<_> = fs::read_dir(dir)?.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir {
            folder.push(name);
            out.folders.push(folder.join("/"));
            walk(&path, folder, out)?;
            folder.pop();
            continue;
        }
        let Some(stem) = name.strip_suffix(".ini").or_else(|| name.strip_suffix(".INI")) else { continue };
        // folder settings, and the defaults for new sessions
        if stem.eq_ignore_ascii_case("__FolderData__") || (folder.is_empty() && stem.eq_ignore_ascii_case("Default")) {
            continue;
        }
        let relative = if folder.is_empty() { name.clone() } else { format!("{}/{name}", folder.join("/")) };
        let bytes = match fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                out.unreadable.push((relative, e.to_string()));
                continue;
            }
        };
        let text = match String::from_utf8(bytes) {
            Ok(t) => t,
            Err(e) => {
                out.not_utf8.push(relative);
                String::from_utf8_lossy(e.as_bytes()).into_owned()
            }
        };
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
        out.sessions.push(session_from(&parse_ini(text), folder.clone(), stem.to_string()));
    }
    Ok(())
}

/// Why a session isn't imported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Skip {
    /// RDP, local shell, Telnet over TLS, …: not something NativeTerm opens.
    Protocol(String),
    NoHostname,
    /// Imported earlier (its `NativeTermSource` is in the config).
    AlreadyImported {
        alias: String,
    },
}

/// A host to write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedHost {
    /// `NativeTermSource` prefix, from the scan's origin.
    pub source_prefix: &'static str,
    pub source: String,
    pub alias: String,
    pub label: String,
    pub hostname: String,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub proxy_jump: Option<String>,
    pub identity_file: Option<String>,
    pub forwards: Vec<Forward>,
    pub options: Vec<(&'static str, String)>,
    pub note: Option<String>,
}

impl PlannedHost {
    /// Directives after `Host <alias>`, in writing order.
    pub fn entries(&self, id: &str) -> Vec<(&'static str, String)> {
        let mut entries = vec![("HostName", self.hostname.clone())];
        if let Some(user) = &self.user {
            entries.push(("User", user.clone()));
        }
        if let Some(port) = self.port.filter(|p| *p != 22) {
            entries.push(("Port", port.to_string()));
        }
        if let Some(jump) = &self.proxy_jump {
            entries.push(("ProxyJump", jump.clone()));
        }
        if let Some(file) = &self.identity_file {
            entries.push(("IdentityFile", file.clone()));
        }
        for forward in &self.forwards {
            entries.push(forward.directive());
        }
        entries.extend(self.options.iter().cloned());
        if self.label != self.alias {
            entries.push(("NativeTermLabel", self.label.clone()));
        }
        if let Some(note) = &self.note {
            entries.push(("NativeTermNote", note.clone()));
        }
        entries.push(("NativeTermId", id.to_string()));
        entries.push(("NativeTermSource", format!("{}{}", self.source_prefix, self.source)));
        entries
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedFolder {
    /// Display name: SecureCRT's folder path joined with " / ".
    pub label: String,
    /// An existing folder file with the same label, appended to; `None`:
    /// a new file.
    pub existing: Option<PathBuf>,
    pub hosts: Vec<PlannedHost>,
    /// Telnet, serial, raw, rlogin and SUPDUP sessions, for the folder's
    /// `.nt.toml`.
    pub plink: Vec<crate::plink::PlinkSession>,
}

impl PlannedFolder {
    pub fn session_count(&self) -> usize {
        self.hosts.len() + self.plink.len()
    }
}

/// Things the user should know before (and after) importing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Notes {
    /// Same host, port and user in several sessions: paths.
    pub duplicates: Vec<Vec<String>>,
    /// Sessions using a global SecureCRT firewall/proxy: name → paths.
    pub named_firewalls: BTreeMap<String, Vec<String>>,
    /// Jump sessions that aren't imported (or don't exist): path → target.
    pub unresolved_jumps: Vec<(String, String)>,
    pub logon_actions: Vec<String>,
    pub saved_passwords: usize,
    /// Non-UTF-8 character sets: path → name. OpenSSH sessions are UTF-8.
    pub encodings: Vec<(String, String)>,
    pub identity_files: usize,
    pub forwards: usize,
    pub bad_forwards: Vec<String>,
    /// Multi-line descriptions joined into one line.
    pub joined_descriptions: usize,
    /// PuTTY keys (`.ppk`) that need converting: path → key file.
    pub ppk_keys: Vec<(String, String)>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub folders: Vec<PlannedFolder>,
    pub skipped: Vec<(String, Skip)>,
    pub notes: Notes,
    /// Host keys to add to `known_hosts` (all found; the writer skips the
    /// ones already there).
    pub host_keys: Vec<crate::known_hosts::HostKey>,
}

impl Plan {
    /// Sessions to write, ssh and non-SSH.
    pub fn host_count(&self) -> usize {
        self.folders.iter().map(PlannedFolder::session_count).sum()
    }
}

/// Label for sessions directly under `Sessions`.
pub const ROOT_FOLDER_LABEL: &str = "SecureCRT";

fn folder_label(folder: &[String], origin: Origin) -> String {
    if folder.is_empty() {
        origin.root_label().to_string()
    } else {
        folder.join(" / ")
    }
}

/// Decide what to write for `scan`, given the current config.
pub fn plan(scan: &Scan, tree: &SessionTree) -> Plan {
    let mut taken = tree.taken_aliases();
    let mut notes = Notes::default();
    let mut skipped = Vec::new();

    // sessions imported earlier from the same program: source path → alias
    let origin = scan.origin;
    let mut imported: HashMap<String, String> = HashMap::new();
    for (_, host) in tree.hosts() {
        if let Some(source) = host.nt.get("source").and_then(|s| s.strip_prefix(origin.source_prefix())) {
            imported.insert(source.to_lowercase(), host.alias().to_string());
        }
    }
    let existing_folders: HashMap<String, PathBuf> =
        tree.folders.iter().map(|f| (f.label().to_lowercase(), f.file.clone())).collect();

    let mut folders: Vec<PlannedFolder> = Vec::new();
    let mut folder_index: HashMap<String, usize> = HashMap::new();
    let mut by_path: HashMap<String, String> = imported.clone();
    let mut pending_jumps: Vec<(usize, usize, String)> = Vec::new();
    let mut seen: HashMap<(String, u16, String), Vec<String>> = HashMap::new();

    for s in &scan.sessions {
        let protocol = s.protocol.to_ascii_lowercase();
        if let Some(alias) = imported.get(&s.path.to_lowercase()) {
            skipped.push((s.path.clone(), Skip::AlreadyImported { alias: alias.clone() }));
            continue;
        }
        let plink_protocol = match protocol.as_str() {
            "ssh2" => None,
            "telnet" => Some(crate::plink::Protocol::Telnet),
            "serial" => Some(crate::plink::Protocol::Serial),
            "raw" => Some(crate::plink::Protocol::Raw),
            "rlogin" => Some(crate::plink::Protocol::Rlogin),
            "supdup" => Some(crate::plink::Protocol::Supdup),
            _ => {
                skipped.push((s.path.clone(), Skip::Protocol(s.protocol.clone())));
                continue;
            }
        };
        if let Some(kind) = plink_protocol {
            let label = folder_label(&s.folder, origin);
            match plink_session(s, kind, origin, &mut taken, &mut notes) {
                Some(session) => {
                    let fi = *folder_index.entry(label.to_lowercase()).or_insert_with(|| {
                        folders.push(PlannedFolder {
                            existing: existing_folders.get(&label.to_lowercase()).cloned(),
                            label: label.clone(),
                            hosts: Vec::new(),
                            plink: Vec::new(),
                        });
                        folders.len() - 1
                    });
                    folders[fi].plink.push(session);
                }
                None => skipped.push((s.path.clone(), Skip::NoHostname)),
            }
            continue;
        }
        let Some(hostname) = s.hostname.clone().filter(|h| !h.contains(char::is_whitespace)) else {
            skipped.push((s.path.clone(), Skip::NoHostname));
            continue;
        };
        let user = s.username.clone().filter(|u| !u.contains(char::is_whitespace));

        let label = folder_label(&s.folder, origin);
        let fi = *folder_index.entry(label.to_lowercase()).or_insert_with(|| {
            folders.push(PlannedFolder {
                existing: existing_folders.get(&label.to_lowercase()).cloned(),
                label: label.clone(),
                hosts: Vec::new(),
                plink: Vec::new(),
            });
            folders.len() - 1
        });

        let base = if alias::sanitize(&s.name) == "host" { hostname.as_str() } else { s.name.as_str() };
        let prefix =
            if s.folder.is_empty() { origin.alias_prefix().to_string() } else { folder_stem(&s.folder.join(" ")) };
        let alias = alias::unique(base, &prefix, &taken);
        taken.insert(alias.to_lowercase());
        by_path.insert(s.path.to_lowercase(), alias.clone());

        let mut proxy_jump = None;
        match &s.firewall {
            Firewall::None => {}
            Firewall::Jump(target) if !target.is_empty() && !target.contains(char::is_whitespace) => {
                proxy_jump = Some(target.clone());
            }
            Firewall::Jump(target) => notes.unresolved_jumps.push((s.path.clone(), target.clone())),
            Firewall::Session(target) => {
                pending_jumps.push((fi, folders[fi].hosts.len(), target.clone()));
            }
            Firewall::Named(name) => notes.named_firewalls.entry(name.clone()).or_default().push(s.path.clone()),
        }
        if s.logon_actions {
            notes.logon_actions.push(s.path.clone());
        }
        if s.saved_password {
            notes.saved_passwords += 1;
        }
        if let Some(encoding) = &s.encoding {
            notes.encodings.push((s.path.clone(), encoding.clone()));
        }
        if s.identity_file.is_some() {
            notes.identity_files += 1;
        }
        if let Some(key) = &s.ppk_key {
            notes.ppk_keys.push((s.path.clone(), key.clone()));
        }
        notes.forwards += s.forwards.len();
        if s.bad_forwards > 0 {
            notes.bad_forwards.push(s.path.clone());
        }
        if s.description.len() > 1 {
            notes.joined_descriptions += 1;
        }
        seen.entry((hostname.to_lowercase(), s.port.unwrap_or(22), user.clone().unwrap_or_default().to_lowercase()))
            .or_default()
            .push(s.path.clone());

        let note = Some(s.description.join(" · ")).filter(|n| !n.is_empty());
        folders[fi].hosts.push(PlannedHost {
            source_prefix: origin.source_prefix(),
            source: s.path.clone(),
            alias,
            label: s.name.clone(),
            hostname,
            user,
            port: s.port,
            proxy_jump,
            identity_file: s.identity_file.clone(),
            forwards: s.forwards.clone(),
            options: s.options.clone(),
            note,
        });
    }

    for (fi, hi, target) in pending_jumps {
        let host = &mut folders[fi].hosts[hi];
        match by_path.get(&target.to_lowercase()) {
            Some(alias) if *alias != host.alias => host.proxy_jump = Some(alias.clone()),
            _ => notes.unresolved_jumps.push((host.source.clone(), target)),
        }
    }

    let mut duplicates: Vec<Vec<String>> = seen.into_values().filter(|paths| paths.len() > 1).collect();
    duplicates.sort();
    notes.duplicates = duplicates;
    Plan { folders, skipped, notes, host_keys: scan.host_keys.keys.clone() }
}

/// A Telnet / serial / raw / rlogin / SUPDUP session to import, or `None`
/// without a host (a serial line for serial).
fn plink_session(
    s: &CrtSession,
    protocol: crate::plink::Protocol,
    origin: Origin,
    taken: &mut HashSet<String>,
    notes: &mut Notes,
) -> Option<crate::plink::PlinkSession> {
    use crate::plink::{PlinkSession, Protocol};
    let plain = |v: &Option<String>| {
        v.clone().map(|x| x.trim().to_string()).filter(|x| !x.is_empty() && !x.contains(char::is_whitespace))
    };
    let (host, serial) = match protocol {
        Protocol::Serial => {
            (None, Some(s.serial.clone().or_else(|| plain(&s.com_port).map(crate::plink::Serial::new))?))
        }
        _ => (Some(plain(&s.hostname)?), None),
    };
    let charset = match s.encoding.as_deref().and_then(crate::plink::charset_from) {
        Some(Ok(charset)) => Some(charset),
        Some(Err(name)) => {
            notes.encodings.push((s.path.clone(), name));
            None
        }
        None => None,
    };
    if s.logon_actions {
        notes.logon_actions.push(s.path.clone());
    }
    if s.saved_password {
        notes.saved_passwords += 1;
    }
    let base = match (&host, &serial) {
        _ if alias::sanitize(&s.name) != "host" => s.name.clone(),
        (Some(h), _) => h.clone(),
        (_, Some(line)) => line.line.clone(),
        _ => s.name.clone(),
    };
    let prefix = if s.folder.is_empty() { origin.alias_prefix().to_string() } else { folder_stem(&s.folder.join(" ")) };
    let name = alias::unique(&base, &prefix, taken);
    taken.insert(name.to_lowercase());
    let session = PlinkSession {
        name: name.clone(),
        label: Some(s.name.clone()).filter(|l| *l != name),
        protocol,
        port: s.port.filter(|p| Some(*p) != protocol.default_port() && host.is_some()),
        host,
        user: plain(&s.username).filter(|_| matches!(protocol, Protocol::Telnet | Protocol::Rlogin)),
        charset,
        serial,
        id: Some(crate::new_id()),
        note: Some(s.description.join(" · ")).filter(|n| !n.is_empty()),
        source: Some(format!("{}{}", origin.source_prefix(), s.path)),
        putty: s.putty.clone(),
        ..PlinkSession::default()
    };
    if session.check().is_err() {
        return None;
    }
    Some(session)
}

/// File stem for a new folder file: ASCII from the label, else `folder`.
pub(crate) fn folder_stem(label: &str) -> String {
    let stem = alias::sanitize(label);
    if stem == "host" {
        "folder".to_string()
    } else {
        stem
    }
}

/// Aliases in the plan must not collide with each other either.
pub fn check_unique(plan: &Plan) -> Result<(), String> {
    let mut seen = HashSet::new();
    let names =
        plan.folders.iter().flat_map(|f| f.hosts.iter().map(|h| &h.alias).chain(f.plink.iter().map(|p| &p.name)));
    for name in names {
        if !seen.insert(name.to_lowercase()) {
            return Err(format!("alias {name} planned twice"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A session file as SecureCRT writes it: BOM, CRLF, binary blocks.
    fn session_file(lines: &[&str]) -> String {
        let mut text = String::from("\u{feff}");
        text.push_str("S:\"Emulation\"=Xterm\r\n");
        text.push_str("B:\"Window Placement\"=0000002c\r\n 2c 00 00 00 00 00 00 00\r\n 70 00 00 00\r\n");
        for l in lines {
            text.push_str(l);
            text.push_str("\r\n");
        }
        text.push_str("D:\"Is Session\"=00000001\r\n");
        text
    }

    fn write(root: &Path, rel: &str, lines: &[&str]) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, session_file(lines)).unwrap();
    }

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let s = dir.path().join("Sessions");
        write(&s, "Default.ini", &["S:\"Hostname\"=", "S:\"Protocol Name\"=SSH2"]);
        write(&s, "__FolderData__.ini", &["S:\"Folder List\"=生产:测试"]);
        write(
            &s,
            "生产/控制节点/10.32.16.66(osp-control1).ini",
            &[
                "S:\"Protocol Name\"=SSH2",
                "S:\"Hostname\"=10.32.16.66",
                "S:\"Username\"=root",
                "D:\"[SSH2] Port\"=00000016",
                "S:\"Firewall Name\"=Session:生产/bastion",
                "Z:\"Description\"=00000002",
                " 控制节点 1",
                " rack 3",
                "S:\"Password V2\"=02:0123abcdef-not-a-real-secret",
                "D:\"Session Password Saved\"=00000001",
                "Z:\"Login Script V3\"=00000002",
                " ogin:",
                " hunter2-not-real",
                "D:\"Use Login Script\"=00000001",
                "Z:\"Port Forward Table V2\"=00000003",
                " web|127.0.0.1,8080|1|10.0.0.9|80||",
                " self|2222|0||22||",
                " dyn|1080|0|socks,|0||",
                "Z:\"Reverse Forward Table V2\"=00000001",
                " back|9000|1|localhost|9001||",
            ],
        );
        write(
            &s,
            "生产/bastion.ini",
            &[
                "S:\"Protocol Name\"=SSH2",
                "S:\"Hostname\"=bastion.example.com",
                "S:\"Username\"=ops",
                "D:\"[SSH2] Port\"=00000d3d",
                "D:\"Use Global Public Key\"=00000000",
                "S:\"Identity Filename V2\"=C:\\Users\\me\\.ssh\\id_ed25519.pub::rawkey",
                "S:\"Output Transformer Name\"=UTF-8",
                "Z:\"Description\"=00000000",
            ],
        );
        write(
            &s,
            "测试/dup.ini",
            &[
                "S:\"Protocol Name\"=SSH2",
                "S:\"Hostname\"=10.32.16.66",
                "S:\"Username\"=root",
                "S:\"Firewall Name\"=Corp Proxy",
                "S:\"Output Transformer Name\"=GBK",
            ],
        );
        write(
            &s,
            "测试/switch.ini",
            &[
                "S:\"Protocol Name\"=Telnet",
                "S:\"Hostname\"=10.1.1.1",
                "D:\"Port\"=00000017",
                "S:\"Username\"=admin",
                "S:\"Output Transformer Name\"=GBK",
            ],
        );
        write(
            &s,
            "测试/console.ini",
            &[
                "S:\"Protocol Name\"=Serial",
                "S:\"Com Port\"=COM3",
                "D:\"Baud Rate\"=0001c200",
                "D:\"Parity\"=00000002",
                "D:\"Stop Bits\"=00000002",
                "D:\"Data Bits\"=00000007",
                "D:\"DTR Flow Control\"=00000001",
                "D:\"RTS Flow Control\"=00000001",
                "D:\"CTS Flow\"=00000001",
            ],
        );
        write(&s, "测试/desk.ini", &["S:\"Protocol Name\"=RDP", "S:\"Hostname\"=pc1"]);
        write(&s, "测试/empty.ini", &["S:\"Protocol Name\"=SSH2", "S:\"Hostname\"="]);
        write(
            &s,
            "root-host.ini",
            &["S:\"Protocol Name\"=SSH2", "S:\"Hostname\"=10.0.0.1", "S:\"Firewall Name\"=Session:gone/away"],
        );
        fs::create_dir_all(s.join("空文件夹")).unwrap();
        dir
    }

    #[test]
    fn secrets_are_never_kept() {
        let text = session_file(&[
            "S:\"Password V2\"=02:abc",
            "S:\"Hostname\"=h",
            "Z:\"Login Script V3\"=00000001",
            " send secret",
            "S:\"Passphrase\"=x",
        ]);
        let ini = parse_ini(&text);
        let dump = format!("{:?}", ini.values);
        assert!(!dump.contains("abc") && !dump.contains("send secret") && !dump.contains("\"x\""), "{dump}");
        assert_eq!(ini.secrets.get("Password V2"), Some(&true));
        assert_eq!(ini.secrets.get("Login Script V3"), Some(&true));
        assert_eq!(ini.str("Hostname"), Some("h"));
        let session = session_from(&ini, vec![], "n".into());
        assert!(format!("{session:?}").find("abc").is_none());
    }

    #[test]
    fn scans_sessions() {
        let dir = fixture();
        let scan = scan(dir.path()).unwrap();
        let paths: Vec<&str> = scan.sessions.iter().map(|s| s.path.as_str()).collect();
        assert_eq!(paths.len(), 8, "{paths:?}");
        assert!(!paths.iter().any(|p| p.contains("Default") || p.contains("__FolderData__")));
        assert!(scan.folders.contains(&"空文件夹".to_string()));

        let control = scan.sessions.iter().find(|s| s.name == "10.32.16.66(osp-control1)").unwrap();
        assert_eq!(control.folder, ["生产", "控制节点"]);
        assert_eq!(control.port, Some(22));
        assert_eq!(control.firewall, Firewall::Session("生产/bastion".into()));
        assert_eq!(control.description, ["控制节点 1", "rack 3"]);
        assert!(control.saved_password && control.logon_actions);
        let directives: Vec<_> = control.forwards.iter().map(Forward::directive).collect();
        assert_eq!(
            directives,
            [
                ("LocalForward", "127.0.0.1:8080 10.0.0.9:80".to_string()),
                ("LocalForward", "2222 localhost:22".to_string()),
                ("DynamicForward", "1080".to_string()),
                ("RemoteForward", "9000 localhost:9001".to_string()),
            ]
        );

        let bastion = scan.sessions.iter().find(|s| s.name == "bastion").unwrap();
        assert_eq!(bastion.port, Some(3389));
        assert_eq!(bastion.identity_file.as_deref(), Some(r"C:\Users\me\.ssh\id_ed25519"));
        assert_eq!(bastion.encoding, None, "UTF-8 is the default");
        assert!(!bastion.saved_password);

        let dup = scan.sessions.iter().find(|s| s.name == "dup").unwrap();
        assert_eq!(dup.firewall, Firewall::Named("Corp Proxy".into()));
        assert_eq!(dup.encoding.as_deref(), Some("GBK"));
        assert_eq!(scan.sessions.iter().find(|s| s.name == "console").unwrap().com_port.as_deref(), Some("COM3"));
    }

    #[test]
    fn plans_an_import() {
        let dir = fixture();
        let scan = scan(dir.path()).unwrap();
        let plan = plan(&scan, &SessionTree::default());
        check_unique(&plan).unwrap();
        let labels: Vec<&str> = plan.folders.iter().map(|f| f.label.as_str()).collect();
        assert_eq!(labels, ["SecureCRT", "测试", "生产", "生产 / 控制节点"]);
        assert_eq!(plan.host_count(), 6, "four ssh hosts, a Telnet and a serial session");

        let hosts: HashMap<&str, &PlannedHost> =
            plan.folders.iter().flat_map(|f| &f.hosts).map(|h| (h.label.as_str(), h)).collect();
        let control = hosts["10.32.16.66(osp-control1)"];
        assert_eq!(control.alias, "10.32.16.66-osp-control1");
        assert_eq!(control.proxy_jump.as_deref(), Some("bastion"));
        assert_eq!(control.note.as_deref(), Some("控制节点 1 · rack 3"));
        let entries = control.entries("id-1");
        assert!(!entries.iter().any(|(k, _)| *k == "Port"), "22 isn't written: {entries:?}");
        assert!(entries.contains(&("NativeTermSource", "securecrt:生产/控制节点/10.32.16.66(osp-control1)".into())));

        let skipped: HashMap<&str, &Skip> = plan.skipped.iter().map(|(p, s)| (p.as_str(), s)).collect();
        assert!(!skipped.contains_key("测试/switch") && !skipped.contains_key("测试/console"));
        let test = plan.folders.iter().find(|f| f.label == "测试").unwrap();
        let switch = test.plink.iter().find(|p| p.label() == "switch").unwrap();
        assert_eq!(switch.protocol, crate::plink::Protocol::Telnet);
        assert_eq!(
            (switch.host.as_deref(), switch.port, switch.user.as_deref()),
            (Some("10.1.1.1"), None, Some("admin"))
        );
        assert_eq!(switch.charset.as_deref(), Some("gbk"));
        assert_eq!(switch.source.as_deref(), Some("securecrt:测试/switch"));
        let console = test.plink.iter().find(|p| p.label() == "console").unwrap();
        assert_eq!(
            console.serial.as_ref().unwrap().sercfg(),
            "115200,7,e,2,R",
            "DTR/RTS are line states, CTS flow is RTS/CTS"
        );
        assert!(console.host.is_none());
        assert_eq!(skipped["测试/desk"], &Skip::Protocol("RDP".into()));
        assert_eq!(skipped["测试/empty"], &Skip::NoHostname);

        let n = &plan.notes;
        assert_eq!(n.duplicates, [vec!["测试/dup".to_string(), "生产/控制节点/10.32.16.66(osp-control1)".to_string()]]);
        assert_eq!(n.named_firewalls["Corp Proxy"], ["测试/dup"]);
        assert_eq!(n.unresolved_jumps, [("root-host".to_string(), "gone/away".to_string())]);
        assert_eq!(n.logon_actions, ["生产/控制节点/10.32.16.66(osp-control1)"]);
        assert_eq!(n.saved_passwords, 1);
        assert_eq!(n.encodings, [("测试/dup".to_string(), "GBK".to_string())], "a Telnet session takes its GBK along");
        assert_eq!((n.identity_files, n.forwards, n.joined_descriptions), (1, 4, 1));
    }

    #[test]
    fn aliases_avoid_existing_hosts() {
        let dir = fixture();
        let scan = scan(dir.path()).unwrap();
        let mut tree = SessionTree::default();
        tree.folders.push(crate::tree::Folder {
            name: "mine".into(),
            file: PathBuf::from("mine.conf"),
            defaults: Default::default(),
            hosts: vec![crate::tree::HostEntry {
                aliases: vec!["bastion".into()],
                hostname: None,
                user: None,
                port: None,
                proxy_jump: None,
                identity_files: vec![],
                nt: Default::default(),
                file: PathBuf::from("mine.conf"),
                line: 1,
                plink: None,
            }],
        });
        let plan = plan(&scan, &tree);
        let bastion = plan.folders.iter().flat_map(|f| &f.hosts).find(|h| h.label == "bastion").unwrap();
        assert_eq!(bastion.alias, "shengchan.bastion", "folder prefix in pinyin");
        let control = plan.folders.iter().flat_map(|f| &f.hosts).find(|h| h.label.starts_with("10.32")).unwrap();
        assert_eq!(control.proxy_jump.as_deref(), Some(bastion.alias.as_str()), "jump follows the new alias");
    }

    #[test]
    fn stems() {
        assert_eq!(folder_stem("生产 / 控制节点"), "shengchan-kongzhijiedian");
        assert_eq!(folder_stem("✓"), "folder");
        assert_eq!(folder_stem("Prod / Ceph"), "prod-ceph");
    }
}
