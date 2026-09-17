//! PuTTY's saved sessions, read (never written) from
//! `HKCU\Software\SimonTatham\PuTTY\Sessions` into the same form as
//! SecureCRT sessions, so the same plan and writer import them. PuTTY
//! stores no passwords.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::PathBuf;

use native_term_win::registry::{self, RegValue};

use crate::securecrt::{CrtSession, Firewall, Forward, ForwardKind, Origin, Scan};

pub const SESSIONS_KEY: &str = r"Software\SimonTatham\PuTTY\Sessions";

/// PuTTY's own defaults, not a session.
const DEFAULT_SETTINGS: &str = "Default Settings";

/// A session's name from its key name: PuTTY writes `%XX` for spaces,
/// `%`, `*`, `?`, `\`, a leading `.` and every non-ASCII byte. The bytes
/// are UTF-8 or, from older versions, the ANSI code page.
pub fn unmunge(name: &str) -> String {
    let bytes = name.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok());
            if let Some(b) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    match String::from_utf8(out) {
        Ok(text) => text,
        Err(e) => registry::from_ansi(e.as_bytes()),
    }
}

/// `PortForwardings`: `L8080=localhost:80,R9000=host:22,D1080`, each with
/// an optional `4`/`6` in front and `bind:` before the port. Returns the
/// forwards and how many entries couldn't be read.
pub fn parse_forwards(value: &str) -> (Vec<Forward>, usize) {
    let mut forwards = Vec::new();
    let mut bad = 0;
    for entry in value.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        match parse_forward(entry) {
            Some(f) => forwards.push(f),
            None => bad += 1,
        }
    }
    (forwards, bad)
}

fn parse_forward(entry: &str) -> Option<Forward> {
    let entry = entry.trim_start_matches(['4', '6']);
    let kind = match entry.chars().next()? {
        'L' => ForwardKind::Local,
        'R' => ForwardKind::Remote,
        'D' => ForwardKind::Dynamic,
        _ => return None,
    };
    let rest = &entry[1..];
    let (listen, target) = match rest.split_once('=') {
        Some((l, t)) => (l, Some(t)),
        None => (rest, None),
    };
    let (bind, port) = match listen.rsplit_once(':') {
        Some((bind, port)) => (Some(bind.trim_matches(['[', ']']).to_string()).filter(|b| !b.is_empty()), port),
        None => (None, listen),
    };
    let port: u16 = port.parse().ok().filter(|p| *p != 0)?;
    let target = match (&kind, target) {
        (ForwardKind::Dynamic, _) => None,
        (_, Some(t)) if t.rsplit_once(':').is_some_and(|(h, p)| !h.is_empty() && p.parse::<u16>().is_ok()) => {
            Some(t.to_string())
        }
        _ => return None,
    };
    Some(Forward { kind, name: entry.to_string(), bind, port, target })
}

struct Values(HashMap<String, RegValue>);

impl Values {
    fn str(&self, key: &str) -> Option<&str> {
        match self.0.get(&key.to_ascii_lowercase()) {
            Some(RegValue::Str(s)) => Some(s.trim()).filter(|s| !s.is_empty()),
            _ => None,
        }
    }

    fn num(&self, key: &str) -> Option<u32> {
        match self.0.get(&key.to_ascii_lowercase()) {
            Some(RegValue::Dword(n)) => Some(*n),
            _ => None,
        }
    }
}

fn protocol_name(protocol: &str) -> String {
    match protocol.to_ascii_lowercase().as_str() {
        "ssh" => "SSH2".into(),
        "telnet" => "Telnet".into(),
        "serial" => "Serial".into(),
        "raw" => "Raw".into(),
        "rlogin" => "RLogin".into(),
        "supdup" => "SUPDUP".into(),
        "bare" => "Bare SSH".into(),
        other => other.to_string(),
    }
}

fn session_from(name: &str, v: &Values, names: &HashSet<String>) -> CrtSession {
    let protocol = protocol_name(v.str("Protocol").unwrap_or("ssh"));
    let ssh = protocol == "SSH2";
    let mut hostname = v.str("HostName").map(str::to_string);
    let mut username = v.str("UserName").map(str::to_string);
    // `user@host` in the host field is allowed too
    if let Some((user, host)) = hostname.as_deref().and_then(|h| h.rsplit_once('@')) {
        if username.is_none() && !user.is_empty() {
            username = Some(user.to_string());
        }
        hostname = Some(host.to_string());
    }
    let port = v.num("PortNumber").and_then(|p| u16::try_from(p).ok()).filter(|p| *p != 0 && ssh);
    let proxy = || {
        let host = v.str("ProxyHost").unwrap_or("");
        let port = v.num("ProxyPort").unwrap_or(0);
        (host.to_string(), port)
    };
    let firewall = match v.num("ProxyMethod").unwrap_or(0) {
        0 => Firewall::None,
        // an SSH jump: a saved session's name, or a host
        6 => {
            let (host, port) = proxy();
            if names.contains(&host.to_lowercase()) {
                Firewall::Session(host)
            } else {
                let user = v.str("ProxyUsername").map(|u| format!("{u}@")).unwrap_or_default();
                let port = if port == 0 || port == 22 { String::new() } else { format!(":{port}") };
                Firewall::Jump(format!("{user}{host}{port}"))
            }
        }
        method => {
            let kind = match method {
                1 => "SOCKS 4",
                2 => "SOCKS 5",
                3 => "HTTP",
                4 => "Telnet",
                5 => "local command",
                _ => "proxy",
            };
            let (host, port) = proxy();
            Firewall::Named(if host.is_empty() { kind.to_string() } else { format!("{kind} {host}:{port}") })
        }
    };
    let (forwards, bad_forwards) = parse_forwards(v.str("PortForwardings").unwrap_or(""));
    let encoding = v
        .str("LineCodePage")
        .filter(|e| !e.to_ascii_lowercase().replace('-', "").contains("utf8"))
        .map(str::to_string);
    let mut options = Vec::new();
    if ssh && v.num("AgentFwd") == Some(1) {
        options.push(("ForwardAgent", "yes".to_string()));
    }
    if ssh && v.num("Compression") == Some(1) {
        options.push(("Compression", "yes".to_string()));
    }
    if let Some(seconds) = v.num("PingIntervalSecs").filter(|s| ssh && *s > 0) {
        options.push(("ServerAliveInterval", seconds.to_string()));
    }
    CrtSession {
        folder: Vec::new(),
        name: name.to_string(),
        path: name.to_string(),
        protocol,
        hostname,
        port,
        username,
        description: Vec::new(),
        firewall,
        identity_file: None,
        forwards,
        bad_forwards,
        encoding,
        com_port: v.str("SerialLine").map(str::to_string),
        logon_actions: false,
        saved_password: false,
        ppk_key: v.str("PublicKeyFile").filter(|_| ssh).map(str::to_string),
        options,
    }
}

/// Where the sessions are read from: PuTTY's key, or for testing the
/// app, `NATIVETERM_PUTTY_KEY` (a key under `HKEY_CURRENT_USER`).
pub fn sessions_key() -> String {
    std::env::var("NATIVETERM_PUTTY_KEY").ok().filter(|k| !k.is_empty()).unwrap_or_else(|| SESSIONS_KEY.to_string())
}

/// PuTTY's saved sessions.
pub fn scan() -> io::Result<Scan> {
    scan_key(&sessions_key())
}

/// Sessions under another key (tests).
pub fn scan_key(key: &str) -> io::Result<Scan> {
    let mut keys: Vec<(String, String)> = registry::user_subkeys(key)?
        .into_iter()
        .map(|k| (unmunge(&k), k))
        .filter(|(name, _)| name != DEFAULT_SETTINGS)
        .collect();
    keys.sort_by_key(|(name, _)| name.to_lowercase());
    let names: HashSet<String> = keys.iter().map(|(n, _)| n.to_lowercase()).collect();
    let mut out = Scan {
        origin: Origin::Putty,
        root: PathBuf::from(format!(r"HKEY_CURRENT_USER\{key}")),
        folders: if keys.is_empty() { Vec::new() } else { vec![String::new()] },
        ..Scan::default()
    };
    for (name, raw) in keys {
        match registry::user_values(&format!(r"{key}\{raw}")) {
            Ok(values) => {
                let values = Values(values.into_iter().map(|(k, v)| (k.to_ascii_lowercase(), v)).collect());
                out.sessions.push(session_from(&name, &values, &names));
            }
            Err(e) => out.unreadable.push((name, e.to_string())),
        }
    }
    Ok(out)
}

/// Whether there is at least one saved session to import.
pub fn has_sessions() -> bool {
    registry::user_subkeys(&sessions_key()).is_ok_and(|keys| keys.iter().any(|k| unmunge(k) != DEFAULT_SETTINGS))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::securecrt::{self, Skip};
    use crate::SessionTree;

    #[test]
    fn names() {
        assert_eq!(unmunge("web%2001"), "web 01");
        assert_eq!(unmunge("%2Ehidden%25x"), ".hidden%x");
        assert_eq!(unmunge("%E6%8E%A7%E5%88%B6"), "控制");
        assert_eq!(unmunge("bad%zz%4"), "bad%zz%4");
        assert_eq!(unmunge("直接"), "直接");
    }

    #[test]
    fn forwards() {
        let (f, bad) = parse_forwards("L8080=localhost:80,4R127.0.0.1:9000=db:5432,D1080,6L[::1]:2222=h:22,X1=y:2,L0=a:1,Lnope");
        assert_eq!(bad, 3);
        let directives: Vec<(&str, String)> = f.iter().map(Forward::directive).collect();
        assert_eq!(
            directives,
            [
                ("LocalForward", "8080 localhost:80".to_string()),
                ("RemoteForward", "127.0.0.1:9000 db:5432".to_string()),
                ("DynamicForward", "1080".to_string()),
                ("LocalForward", "[::1]:2222 h:22".to_string()),
            ]
        );
    }

    fn s(v: &str) -> RegValue {
        RegValue::Str(v.into())
    }

    /// Sessions in a test key; deleted again when dropped.
    struct TestKey(String);

    impl Drop for TestKey {
        fn drop(&mut self) {
            let _ = registry::delete_user_tree(&self.0);
        }
    }

    fn fixture(test: &str) -> TestKey {
        let key = format!(r"Software\NativeTerm-Tests-putty-{test}-{}", std::process::id());
        let _ = registry::delete_user_tree(&key);
        let session = |name: &str, values: &[(&str, RegValue)]| {
            registry::write_user_values(&format!(r"{key}\{name}"), values).unwrap();
        };
        session("Default%20Settings", &[("HostName", s("")), ("Protocol", s("ssh"))]);
        session(
            "%E6%8E%A7%E5%88%B6%E8%8A%82%E7%82%B9",
            &[
                ("HostName", s("ops@10.32.16.66")),
                ("PortNumber", RegValue::Dword(2222)),
                ("Protocol", s("ssh")),
                ("PortForwardings", s("L8080=localhost:80,D1080,Lbad")),
                ("AgentFwd", RegValue::Dword(1)),
                ("PingIntervalSecs", RegValue::Dword(30)),
                ("ProxyMethod", RegValue::Dword(6)),
                ("ProxyHost", s("Bastion")),
                ("PublicKeyFile", s(r"C:\keys\me.ppk")),
            ],
        );
        session("bastion", &[("HostName", s("bastion.example.com")), ("UserName", s("jump")), ("Protocol", s("ssh"))]);
        session(
            "via%20host",
            &[
                ("HostName", s("db.internal")),
                ("Protocol", s("ssh")),
                ("ProxyMethod", RegValue::Dword(6)),
                ("ProxyHost", s("gw.example.com")),
                ("ProxyPort", RegValue::Dword(2200)),
                ("ProxyUsername", s("me")),
                ("LineCodePage", s("GBK")),
            ],
        );
        session(
            "socks",
            &[
                ("HostName", s("x.example.com")),
                ("Protocol", s("ssh")),
                ("ProxyMethod", RegValue::Dword(2)),
                ("ProxyHost", s("proxy")),
                ("ProxyPort", RegValue::Dword(1080)),
                ("LineCodePage", s("UTF-8")),
            ],
        );
        session("switch", &[("HostName", s("10.1.1.1")), ("Protocol", s("telnet")), ("PortNumber", RegValue::Dword(23))]);
        session("console", &[("Protocol", s("serial")), ("SerialLine", s("COM3"))]);
        session("empty", &[("Protocol", s("ssh"))]);
        TestKey(key)
    }

    #[test]
    fn scan_and_plan() {
        let key = fixture("scan");
        let scan = scan_key(&key.0).unwrap();
        let names: Vec<&str> = scan.sessions.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["bastion", "console", "empty", "socks", "switch", "via host", "控制节点"]);
        let control = scan.sessions.iter().find(|s| s.name == "控制节点").unwrap();
        assert_eq!(control.hostname.as_deref(), Some("10.32.16.66"));
        assert_eq!(control.username.as_deref(), Some("ops"));
        assert_eq!(control.port, Some(2222));
        assert_eq!(control.firewall, Firewall::Session("Bastion".into()));
        assert_eq!((control.forwards.len(), control.bad_forwards), (2, 1));

        let tree = SessionTree::default();
        let plan = securecrt::plan(&scan, &tree);
        assert_eq!(plan.folders.len(), 1);
        assert_eq!(plan.folders[0].label, "PuTTY");
        let hosts = &plan.folders[0].hosts;
        let host = |label: &str| hosts.iter().find(|h| h.label == label).unwrap();
        let bastion = host("bastion").alias.clone();
        assert_eq!(bastion, "bastion");
        let control = host("控制节点");
        assert_eq!(control.alias, "kongzhijiedian");
        assert_eq!(control.proxy_jump.as_deref(), Some(bastion.as_str()), "jump by session name, any case");
        let entries = control.entries("id-1");
        for wanted in [
            ("ForwardAgent", "yes"),
            ("ServerAliveInterval", "30"),
            ("LocalForward", "8080 localhost:80"),
            ("DynamicForward", "1080"),
            ("NativeTermSource", "putty:控制节点"),
        ] {
            assert!(entries.iter().any(|(k, v)| *k == wanted.0 && v == wanted.1), "{wanted:?} in {entries:?}");
        }
        assert!(!entries.iter().any(|(k, _)| *k == "IdentityFile"), "a .ppk key isn't usable");
        assert_eq!(host("via host").proxy_jump.as_deref(), Some("me@gw.example.com:2200"));
        assert!(host("socks").proxy_jump.is_none());

        let n = &plan.notes;
        assert_eq!(n.ppk_keys, [("控制节点".to_string(), r"C:\keys\me.ppk".to_string())]);
        assert_eq!(n.named_firewalls.keys().collect::<Vec<_>>(), ["SOCKS 5 proxy:1080"]);
        assert_eq!(n.encodings, [("via host".to_string(), "GBK".to_string())]);
        assert_eq!(n.bad_forwards, ["控制节点"]);
        let skipped: Vec<(&str, &Skip)> = plan.skipped.iter().map(|(p, s)| (p.as_str(), s)).collect();
        assert_eq!(
            skipped,
            [
                ("console", &Skip::PlinkLater("Serial".into())),
                ("empty", &Skip::NoHostname),
                ("switch", &Skip::PlinkLater("Telnet".into())),
            ]
        );
    }

    #[test]
    fn missing_key_is_empty() {
        let scan = scan_key(r"Software\NativeTerm-Tests-no-such-putty").unwrap();
        assert!(scan.sessions.is_empty() && scan.folders.is_empty());
    }

    /// Written with the editor, a second import skips everything.
    #[test]
    fn import_round_trip() {
        let ssh = PathBuf::from(r"C:\Windows\System32\OpenSSH\ssh.exe");
        if !ssh.exists() {
            return;
        }
        let key = fixture("import");
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join(".ssh");
        std::fs::create_dir_all(&dir).unwrap();
        let editor = crate::ops::Editor::for_directory(&dir, crate::write::Writer::new(home.path().join("backups")), &ssh);
        std::fs::write(editor.main_config(), "").unwrap();
        crate::acl::restrict_to_owner(&editor.main_config()).unwrap();
        let scan = scan_key(&key.0).unwrap();
        let plan = securecrt::plan(&scan, &SessionTree::load(&dir));
        let outcome = editor.import(&plan, &|_, _| {}).unwrap();
        assert_eq!(outcome.hosts(), 4, "{:?}", outcome.failed);

        let tree = SessionTree::load(&dir);
        let (folder, host) = tree.find("via-host").unwrap();
        assert_eq!(folder.label(), "PuTTY");
        assert_eq!(host.proxy_jump.as_deref(), Some("me@gw.example.com:2200"));
        let again = securecrt::plan(&scan, &tree);
        assert_eq!(again.host_count(), 0);
        assert_eq!(again.skipped.iter().filter(|(_, s)| matches!(s, Skip::AlreadyImported { .. })).count(), 4);
    }
}
