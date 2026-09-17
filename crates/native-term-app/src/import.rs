//! Importing from SecureCRT: where its sessions are, and the summary shown
//! before anything is written.

use std::collections::BTreeMap;
use std::path::PathBuf;

use native_term_config::securecrt::{Plan, Scan, Skip};

/// SecureCRT's configuration folder (`HKCU\Software\VanDyke\SecureCRT`,
/// `Config Path`), if it is installed and the folder exists.
pub fn securecrt_config_path() -> Option<PathBuf> {
    let value = native_term_win::desktop::user_registry_string(r"Software\VanDyke\SecureCRT", "Config Path").ok()??;
    let path = PathBuf::from(value);
    path.join("Sessions").is_dir().then_some(path)
}

/// One line of the summary; `detail` lines list session paths.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub text: String,
    pub warning: bool,
    pub details: Vec<String>,
}

fn line(text: String, warning: bool, details: Vec<String>) -> Line {
    Line { text, warning, details }
}

/// What an import of `plan` would do, in words.
pub fn summary(scan: &Scan, plan: &Plan) -> Vec<Line> {
    let mut out = Vec::new();
    let new_folders = plan.folders.iter().filter(|f| f.existing.is_none() && !f.hosts.is_empty()).count();
    let existing = plan.folders.iter().filter(|f| f.existing.is_some() && !f.hosts.is_empty()).count();
    out.push(line(
        format!(
            "{} sessions found in {} folders; {} will be imported into {} new and {} existing folders",
            scan.sessions.len(),
            scan.folders.len(),
            plan.host_count(),
            new_folders,
            existing
        ),
        false,
        plan.folders
            .iter()
            .filter(|f| !f.hosts.is_empty())
            .map(|f| format!("{} ({})", f.label, f.hosts.len()))
            .collect(),
    ));

    let mut skipped: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (path, why) in &plan.skipped {
        let reason = match why {
            Skip::AlreadyImported { .. } => "already imported".to_string(),
            Skip::PlinkLater(p) => format!("{p}: not supported yet (planned through plink)"),
            Skip::Protocol(p) => format!("{p}: not a terminal session NativeTerm opens"),
            Skip::NoHostname => "no host name".to_string(),
        };
        skipped.entry(reason).or_default().push(path.clone());
    }
    for (reason, paths) in skipped {
        let warning = !reason.starts_with("already");
        out.push(line(format!("Skipped, {reason}: {}", paths.len()), warning, paths));
    }

    let n = &plan.notes;
    if !n.duplicates.is_empty() {
        out.push(line(
            format!("Same host, port and user in several sessions: {} groups (all imported; review them)", n.duplicates.len()),
            true,
            n.duplicates.iter().map(|g| g.join("  =  ")).collect(),
        ));
    }
    for (name, paths) in &n.named_firewalls {
        out.push(line(
            format!("Firewall/proxy \u{201c}{name}\u{201d} isn't imported: {} sessions connect directly", paths.len()),
            true,
            paths.clone(),
        ));
    }
    if !n.unresolved_jumps.is_empty() {
        out.push(line(
            format!("Jump session not found or not imported: {} sessions connect directly", n.unresolved_jumps.len()),
            true,
            n.unresolved_jumps.iter().map(|(p, t)| format!("{p}  →  {t}")).collect(),
        ));
    }
    if !n.logon_actions.is_empty() {
        out.push(line(
            format!("Logon actions/scripts aren't imported: {} sessions", n.logon_actions.len()),
            true,
            n.logon_actions.clone(),
        ));
    }
    if n.saved_passwords > 0 {
        out.push(line(
            format!("Saved passwords aren't imported ({} sessions); use keys or ssh-agent", n.saved_passwords),
            true,
            Vec::new(),
        ));
    }
    if !n.encodings.is_empty() {
        out.push(line(
            format!("Non-UTF-8 character sets (OpenSSH sessions are UTF-8): {} sessions", n.encodings.len()),
            true,
            n.encodings.iter().map(|(p, e)| format!("{p}  ({e})")).collect(),
        ));
    }
    if !n.bad_forwards.is_empty() {
        out.push(line(
            format!("Unreadable port forwards left out: {} sessions", n.bad_forwards.len()),
            true,
            n.bad_forwards.clone(),
        ));
    }
    let mut kept = Vec::new();
    if n.forwards > 0 {
        kept.push(format!("{} port forwards", n.forwards));
    }
    if n.identity_files > 0 {
        kept.push(format!("{} key files (must be in OpenSSH format)", n.identity_files));
    }
    if n.joined_descriptions > 0 {
        kept.push(format!("{} multi-line descriptions joined into one line", n.joined_descriptions));
    }
    if !kept.is_empty() {
        out.push(line(format!("Also imported: {}", kept.join(", ")), false, Vec::new()));
    }
    if !scan.unreadable.is_empty() {
        out.push(line(
            format!("Unreadable session files: {}", scan.unreadable.len()),
            true,
            scan.unreadable.iter().map(|(p, e)| format!("{p}: {e}")).collect(),
        ));
    }
    if !scan.not_utf8.is_empty() {
        out.push(line(
            format!("Session files that aren't UTF-8 (read with replacement characters): {}", scan.not_utf8.len()),
            true,
            scan.not_utf8.clone(),
        ));
    }
    out.push(line(
        "Not imported yet: host keys (KnownHosts) and saved commands".to_string(),
        false,
        Vec::new(),
    ));
    out
}
