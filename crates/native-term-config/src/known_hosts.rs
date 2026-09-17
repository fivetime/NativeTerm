//! Host keys: reading SecureCRT's `KnownHosts` folder and adding keys to
//! OpenSSH's `known_hosts`, so imported hosts don't each ask for their
//! fingerprint on the first connect.
//!
//! SecureCRT's format isn't documented. What is read here, leniently:
//! - one key per `.pub` file, named `<name>[<address>]<port>.pub`;
//! - the key as an OpenSSH line (`ssh-ed25519 AAAA… comment`) or as an
//!   RFC 4716 block (`---- BEGIN SSH2 PUBLIC KEY ----`).
//!
//! Files that don't fit are counted and reported, never guessed at.

use std::fs;
use std::io;
use std::path::Path;

/// One host key, as OpenSSH wants it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostKey {
    /// Names the key is known under (the address, and the session's name
    /// when it differs).
    pub hosts: Vec<String>,
    pub port: u16,
    pub key_type: String,
    /// Base64 of the key blob.
    pub key: String,
}

impl HostKey {
    /// The `known_hosts` line.
    pub fn line(&self) -> String {
        let names: Vec<String> = self
            .hosts
            .iter()
            .map(|h| if self.port == 22 { h.clone() } else { format!("[{h}]:{}", self.port) })
            .collect();
        format!("{} {} {}", names.join(","), self.key_type, self.key)
    }
}

#[derive(Debug, Default)]
pub struct KeyScan {
    pub keys: Vec<HostKey>,
    /// `.pub` files whose name or content wasn't understood.
    pub not_understood: usize,
    /// Other files (maps, databases) that were left alone.
    pub other_files: usize,
}

/// `name[address]port` → (name, address, port).
fn parse_name(stem: &str) -> Option<(String, String, u16)> {
    let open = stem.rfind('[')?;
    let close = stem[open..].find(']')? + open;
    let name = stem[..open].trim();
    let address = stem[open + 1..close].trim();
    let port: u16 = stem[close + 1..].trim().parse().ok().filter(|p| *p != 0)?;
    let valid = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':'));
    if !valid(address) {
        return None;
    }
    let name = if valid(name) { name } else { address };
    Some((name.to_string(), address.to_string(), port))
}

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0;
    for c in text.bytes().filter(|c| !c.is_ascii_whitespace()) {
        if c == b'=' {
            break;
        }
        let value = BASE64.iter().position(|b| *b == c)? as u32;
        buffer = (buffer << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Some(out)
}

/// The key type named at the start of an SSH key blob.
fn blob_type(blob: &[u8]) -> Option<String> {
    let len = u32::from_be_bytes(blob.get(..4)?.try_into().ok()?) as usize;
    let name = std::str::from_utf8(blob.get(4..4 + len)?).ok()?;
    (name.starts_with("ssh-") || name.starts_with("ecdsa-") || name.starts_with("sk-")).then(|| name.to_string())
}

/// (type, base64) from a key file's text.
fn parse_key(text: &str) -> Option<(String, String)> {
    let text = text.trim_start_matches('\u{feff}');
    if text.contains("BEGIN SSH2 PUBLIC KEY") {
        let body: String = text
            .lines()
            .filter(|l| !l.contains("----") && !l.contains(':'))
            .map(str::trim)
            .collect();
        let blob = base64_decode(&body)?;
        return Some((blob_type(&blob)?, body));
    }
    let line = text.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with('#'))?;
    let mut fields = line.split_whitespace();
    let (key_type, key) = (fields.next()?, fields.next()?);
    let blob = base64_decode(key)?;
    // the blob names its own type; they must agree
    (blob_type(&blob)? == key_type).then(|| (key_type.to_string(), key.to_string()))
}

/// Read `<config>\KnownHosts` (missing folder: nothing found).
pub fn scan_securecrt(config: &Path) -> io::Result<KeyScan> {
    let dir = ["KnownHosts", "Known_hosts", "knownhosts"].iter().map(|n| config.join(n)).find(|d| d.is_dir());
    let mut out = KeyScan::default();
    let Some(dir) = dir else { return Ok(out) };
    let mut entries: Vec<_> = fs::read_dir(&dir)?.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().to_string();
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let Some(stem) = name.strip_suffix(".pub") else {
            out.other_files += 1;
            continue;
        };
        let parsed = parse_name(stem).and_then(|(host, address, port)| {
            let text = fs::read_to_string(entry.path()).ok()?;
            let (key_type, key) = parse_key(&text)?;
            let mut hosts = vec![address.clone()];
            if host != address {
                hosts.push(host);
            }
            Some(HostKey { hosts, port, key_type, key })
        });
        match parsed {
            Some(key) => out.keys.push(key),
            None => out.not_understood += 1,
        }
    }
    Ok(out)
}

/// The keys not yet in `existing` (a `known_hosts` text): same key blob
/// for the same host name. Hashed entries can't be matched and don't
/// count.
pub fn missing<'k>(existing: &str, keys: &'k [HostKey]) -> Vec<&'k HostKey> {
    let known: Vec<(Vec<String>, String)> = existing
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with('@') && !l.starts_with('|'))
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            let names = f.next()?.split(',').map(|n| n.to_ascii_lowercase()).collect();
            let _type = f.next()?;
            Some((names, f.next()?.to_string()))
        })
        .collect();
    keys.iter()
        .filter(|k| {
            let names: Vec<String> = k
                .line()
                .split_whitespace()
                .next()
                .unwrap_or("")
                .split(',')
                .map(|n| n.to_ascii_lowercase())
                .collect();
            !known.iter().any(|(existing, blob)| *blob == k.key && names.iter().any(|n| existing.contains(n)))
        })
        .collect()
}

/// How `known_hosts` names a host: `host`, or `[host]:port`.
pub fn host_name(host: &str, port: u16) -> String {
    if port == 22 {
        host.to_string()
    } else {
        format!("[{host}]:{port}")
    }
}

/// Remove every key for `names` with `ssh-keygen -R` (which also finds
/// hashed entries and keeps `known_hosts.old`). Returns the names that had
/// keys.
pub fn remove(ssh_keygen: &Path, known_hosts: &Path, names: &[String]) -> io::Result<Vec<String>> {
    let mut removed = Vec::new();
    if !known_hosts.exists() {
        return Ok(removed);
    }
    for name in names {
        if name.is_empty() || name.starts_with('-') {
            continue;
        }
        let mut command = std::process::Command::new(ssh_keygen);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // no console window
        }
        let output = command
            .arg("-R")
            .arg(name)
            .arg("-f")
            .arg(known_hosts)
            .stdin(std::process::Stdio::null())
            .output()?;
        if !output.status.success() {
            let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(io::Error::other(format!("ssh-keygen -R {name}: {message}")));
        }
        // "# Host <name> found: line <n>" for each removed line
        if String::from_utf8_lossy(&output.stdout).contains("found") {
            removed.push(name.clone());
        }
    }
    Ok(removed)
}

/// `ssh-keygen` next to the `ssh` NativeTerm uses.
pub fn ssh_keygen_for(ssh: &Path) -> std::path::PathBuf {
    match ssh.parent().filter(|p| !p.as_os_str().is_empty()) {
        Some(dir) if dir.join("ssh-keygen.exe").exists() => dir.join("ssh-keygen.exe"),
        _ => std::path::PathBuf::from("ssh-keygen"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // a real ed25519 public key blob (type string + 32 bytes)
    const ED25519: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl";

    #[test]
    #[cfg(windows)]
    fn removes_with_ssh_keygen() {
        let keygen = Path::new(r"C:\Windows\System32\OpenSSH\ssh-keygen.exe");
        if !keygen.exists() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("known_hosts");
        fs::write(&file, format!("10.0.0.5 ssh-ed25519 {ED25519}
[10.0.0.6]:2200 ssh-ed25519 {ED25519}
keep ssh-ed25519 {ED25519}
"))
            .unwrap();
        let names = vec![host_name("10.0.0.5", 22), host_name("10.0.0.6", 2200), host_name("absent", 22)];
        assert_eq!(remove(keygen, &file, &names).unwrap(), ["10.0.0.5", "[10.0.0.6]:2200"]);
        let left = fs::read_to_string(&file).unwrap();
        assert!(left.contains("keep") && !left.contains("10.0.0.5") && !left.contains("10.0.0.6"), "{left}");
        assert!(dir.path().join("known_hosts.old").exists());
    }

    #[test]
    fn names() {
        assert_eq!(parse_name("web01[10.0.0.5]22"), Some(("web01".into(), "10.0.0.5".into(), 22)));
        assert_eq!(parse_name("10.0.0.5[10.0.0.5]2222"), Some(("10.0.0.5".into(), "10.0.0.5".into(), 2222)));
        assert_eq!(parse_name("控制节点[10.0.0.6]22").unwrap().0, "10.0.0.6", "unusable name: the address");
        assert_eq!(parse_name("hostsmap"), None);
        assert_eq!(parse_name("x[10.0.0.5]"), None);
    }

    #[test]
    fn key_formats() {
        assert_eq!(parse_key(&format!("ssh-ed25519 {ED25519} comment\n")), Some(("ssh-ed25519".into(), ED25519.into())));
        let rfc = format!("---- BEGIN SSH2 PUBLIC KEY ----\nComment: \"x\"\n{}\n{}\n---- END SSH2 PUBLIC KEY ----\n", &ED25519[..40], &ED25519[40..]);
        assert_eq!(parse_key(&rfc), Some(("ssh-ed25519".into(), ED25519.into())));
        assert_eq!(parse_key(&format!("ssh-rsa {ED25519}")), None, "type and blob disagree");
        assert_eq!(parse_key("not a key"), None);
    }

    #[test]
    fn scans_a_folder_and_finds_what_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let known = dir.path().join("KnownHosts");
        fs::create_dir_all(&known).unwrap();
        fs::write(known.join("web01[10.0.0.5]22.pub"), format!("ssh-ed25519 {ED25519}\n")).unwrap();
        fs::write(known.join("db[10.0.0.6]2200.pub"), format!("ssh-ed25519 {ED25519}\n")).unwrap();
        fs::write(known.join("broken[10.0.0.7]22.pub"), "garbage").unwrap();
        fs::write(known.join("hostsmap.txt"), "whatever").unwrap();
        let scan = scan_securecrt(dir.path()).unwrap();
        assert_eq!((scan.keys.len(), scan.not_understood, scan.other_files), (2, 1, 1));
        let lines: Vec<String> = scan.keys.iter().map(HostKey::line).collect();
        assert_eq!(lines[0], format!("[10.0.0.6]:2200,[db]:2200 ssh-ed25519 {ED25519}"));
        assert_eq!(lines[1], format!("10.0.0.5,web01 ssh-ed25519 {ED25519}"));

        let existing = format!("# mine\n10.0.0.5 ssh-ed25519 {ED25519}\n|1|hashed= ssh-rsa AAAA\n");
        let todo = missing(&existing, &scan.keys);
        assert_eq!(todo.len(), 1);
        assert_eq!(todo[0].port, 2200);
        assert!(scan_securecrt(&dir.path().join("nothing")).unwrap().keys.is_empty());
    }
}
