//! Session options: the ssh settings NativeTerm edits beyond the host
//! dialog, grouped like SecureCRT's "Session Options". Values are kept as
//! written after the keyword (quotes included) and checked by `ssh -G`
//! when saved, so NativeTerm never has to understand every option.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::document::Document;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Connection,
    Authentication,
    Algorithms,
    HostKey,
    Forwarding,
    Environment,
}

impl Category {
    pub const ALL: [Category; 6] = [
        Category::Connection,
        Category::Authentication,
        Category::Algorithms,
        Category::HostKey,
        Category::Forwarding,
        Category::Environment,
    ];
}

/// Where the names of a list option come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Names {
    /// `ssh -Q <query>` on this machine.
    Query(&'static str),
    Fixed(&'static [&'static str]),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// One value, typed.
    Text,
    /// One of these words.
    Choice(&'static [&'static str]),
    /// A comma-separated list of names (ssh's `+`/`-`/`^` prefixes allowed).
    List(Names),
    /// Any number of directives, one per line.
    Lines,
}

pub struct Spec {
    pub keyword: &'static str,
    pub category: Category,
    pub kind: Kind,
    /// Only offered for folders (the host dialog has these for hosts).
    pub folder_only: bool,
}

const YES_NO: &[&str] = &["yes", "no"];

const fn spec(keyword: &'static str, category: Category, kind: Kind) -> Spec {
    Spec { keyword, category, kind, folder_only: false }
}

const fn folder_spec(keyword: &'static str, category: Category, kind: Kind) -> Spec {
    Spec { keyword, category, kind, folder_only: true }
}

use Category::*;

pub const SPECS: &[Spec] = &[
    folder_spec("User", Connection, Kind::Text),
    folder_spec("Port", Connection, Kind::Text),
    folder_spec("ProxyJump", Connection, Kind::Text),
    spec("ConnectTimeout", Connection, Kind::Text),
    spec("ServerAliveInterval", Connection, Kind::Text),
    spec("ServerAliveCountMax", Connection, Kind::Text),
    spec("TCPKeepAlive", Connection, Kind::Choice(YES_NO)),
    spec("Compression", Connection, Kind::Choice(YES_NO)),
    spec("AddressFamily", Connection, Kind::Choice(&["any", "inet", "inet6"])),
    spec("RequestTTY", Connection, Kind::Choice(&["auto", "yes", "no", "force"])),
    spec("RemoteCommand", Connection, Kind::Text),
    spec(
        "LogLevel",
        Connection,
        Kind::Choice(&["QUIET", "FATAL", "ERROR", "INFO", "VERBOSE", "DEBUG1", "DEBUG2", "DEBUG3"]),
    ),
    folder_spec("IdentityFile", Authentication, Kind::Lines),
    spec(
        "PreferredAuthentications",
        Authentication,
        Kind::List(Names::Fixed(&["publickey", "keyboard-interactive", "password", "gssapi-with-mic", "hostbased"])),
    ),
    spec("PubkeyAuthentication", Authentication, Kind::Choice(YES_NO)),
    spec("PasswordAuthentication", Authentication, Kind::Choice(YES_NO)),
    spec("KbdInteractiveAuthentication", Authentication, Kind::Choice(YES_NO)),
    spec("IdentitiesOnly", Authentication, Kind::Choice(YES_NO)),
    spec("ForwardAgent", Authentication, Kind::Choice(YES_NO)),
    spec("PubkeyAcceptedAlgorithms", Authentication, Kind::List(Names::Query("key-sig"))),
    spec("KexAlgorithms", Algorithms, Kind::List(Names::Query("kex"))),
    spec("Ciphers", Algorithms, Kind::List(Names::Query("cipher"))),
    spec("MACs", Algorithms, Kind::List(Names::Query("mac"))),
    spec("HostKeyAlgorithms", Algorithms, Kind::List(Names::Query("key-sig"))),
    spec("StrictHostKeyChecking", HostKey, Kind::Choice(&["ask", "accept-new", "yes", "no"])),
    spec("UpdateHostKeys", HostKey, Kind::Choice(&["yes", "no", "ask"])),
    spec("CheckHostIP", HostKey, Kind::Choice(YES_NO)),
    spec("LocalForward", Forwarding, Kind::Lines),
    spec("RemoteForward", Forwarding, Kind::Lines),
    spec("DynamicForward", Forwarding, Kind::Lines),
    spec("ExitOnForwardFailure", Forwarding, Kind::Choice(YES_NO)),
    spec("GatewayPorts", Forwarding, Kind::Choice(YES_NO)),
    spec("ForwardX11", Forwarding, Kind::Choice(YES_NO)),
    spec("ForwardX11Trusted", Forwarding, Kind::Choice(YES_NO)),
    spec("SetEnv", Environment, Kind::Lines),
    spec("SendEnv", Environment, Kind::Lines),
];

pub fn find(keyword: &str) -> Option<&'static Spec> {
    SPECS.iter().find(|s| s.keyword.eq_ignore_ascii_case(keyword))
}

/// Values per keyword, as written (one entry per directive line); every
/// keyword of [`SPECS`] is present, empty when not set.
pub type Values = BTreeMap<&'static str, Vec<String>>;

pub fn empty() -> Values {
    SPECS.iter().map(|s| (s.keyword, Vec::new())).collect()
}

/// The options written in a block (not inherited ones).
pub fn read(doc: &Document, block: usize) -> Values {
    let mut values = empty();
    let b = doc.blocks().swap_remove(block);
    for (i, d) in doc.directives(&b) {
        if let (Some(spec), Some(raw)) = (find(&d.keyword), doc.lines[i].raw_value()) {
            values.entry(spec.keyword).or_default().push(raw.to_string());
        }
    }
    values
}

/// Trimmed, empty entries dropped, one value for single-valued options;
/// line breaks and unknown keywords are refused.
pub fn normalize(values: &Values) -> Result<Values, String> {
    let mut out = Values::new();
    for (keyword, list) in values {
        let spec = find(keyword).ok_or_else(|| format!("unknown option {keyword}"))?;
        if list.iter().any(|v| v.contains(['\r', '\n'])) {
            return Err(format!("{keyword}: one value per line"));
        }
        let mut list: Vec<String> = list.iter().map(|v| v.trim().to_string()).filter(|v| !v.is_empty()).collect();
        if spec.kind != Kind::Lines {
            list.truncate(1);
        }
        out.insert(spec.keyword, list);
    }
    Ok(out)
}

/// Write `values` into a block; keywords whose values are unchanged are
/// left alone (lines, spelling and order kept), keywords not in `values`
/// too.
pub fn apply(doc: &mut Document, block: usize, values: &Values) {
    let current = read(doc, block);
    for (keyword, list) in values {
        if current.get(keyword) != Some(list) {
            doc.set_raw_values(block, keyword, list);
        }
    }
}

/// The names `ssh -Q <what>` lists, e.g. the ciphers this ssh supports.
pub fn query(ssh: &Path, what: &str) -> Vec<String> {
    let mut command = Command::new(ssh);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // no console window
    }
    match command.arg("-Q").arg(what).stdin(Stdio::null()).stderr(Stdio::null()).output() {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect()
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = "\
# mine
Host web
    HostName 10.0.0.1
    compression=yes
    LocalForward 8080 localhost:80
    # keep me
    LocalForward 8443 localhost:443
    RemoteCommand tmux new -A -s \"main\"
    NativeTermNote x
";

    #[test]
    fn reads_values_as_written() {
        let doc = Document::parse(CONFIG);
        let block = doc.find_host_block("web").unwrap();
        let values = read(&doc, block);
        assert_eq!(values["Compression"], ["yes"]);
        assert_eq!(values["LocalForward"], ["8080 localhost:80", "8443 localhost:443"]);
        assert_eq!(values["RemoteCommand"], ["tmux new -A -s \"main\""]);
        assert!(values["Ciphers"].is_empty());
        assert_eq!(values.len(), SPECS.len());
    }

    #[test]
    fn applies_only_changes() {
        let mut doc = Document::parse(CONFIG);
        let block = doc.find_host_block("web").unwrap();
        let mut values = read(&doc, block);
        values.insert("Compression", vec!["no".into()]);
        values.insert("LocalForward", vec!["9090 localhost:90".into()]);
        values.insert("Ciphers", vec!["+aes128-cbc".into()]);
        values.insert("SetEnv", vec!["TERM=xterm-256color".into(), "LANG=\"zh_CN.UTF-8\"".into()]);
        let values = normalize(&values).unwrap();
        apply(&mut doc, block, &values);
        assert_eq!(
            doc.render(),
            "\
# mine
Host web
    HostName 10.0.0.1
    compression=no
    LocalForward 9090 localhost:90
    # keep me
    RemoteCommand tmux new -A -s \"main\"
    NativeTermNote x
    Ciphers +aes128-cbc
    SetEnv TERM=xterm-256color
    SetEnv LANG=\"zh_CN.UTF-8\"
"
        );
        assert_eq!(read(&doc, block), values);

        // clearing removes the lines
        let mut cleared = values.clone();
        cleared.insert("SetEnv", Vec::new());
        cleared.insert("RemoteCommand", vec!["  ".into()]);
        apply(&mut doc, block, &normalize(&cleared).unwrap());
        let text = doc.render();
        assert!(!text.contains("SetEnv") && !text.contains("RemoteCommand"), "{text}");
    }

    #[test]
    fn normalize_refuses_bad_input() {
        let mut values = empty();
        values.insert("Ciphers", vec!["a\nb".into()]);
        assert!(normalize(&values).is_err());
        let mut values = empty();
        values.insert("Ciphers", vec!["a".into(), "b".into()]);
        assert_eq!(normalize(&values).unwrap()["Ciphers"], ["a"]);
    }

    #[test]
    fn ssh_lists_algorithms() {
        let ssh = Path::new(r"C:\Windows\System32\OpenSSH\ssh.exe");
        if ssh.exists() {
            assert!(query(ssh, "cipher").iter().any(|c| c == "aes256-ctr"));
            assert!(query(ssh, "no-such-list").is_empty());
        }
    }
}
