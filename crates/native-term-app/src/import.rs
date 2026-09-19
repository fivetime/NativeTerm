//! Importing from SecureCRT or PuTTY: where the sessions are, and the
//! summary shown before anything is written.

use std::collections::BTreeMap;
use std::path::PathBuf;

use native_term_config::securecrt::{Origin, Plan, Scan, Skip};

use crate::t;

/// SecureCRT's configuration folder (`HKCU\Software\VanDyke\SecureCRT`,
/// `Config Path`), if it is installed and the folder exists.
pub fn securecrt_config_path() -> Option<PathBuf> {
    // tests: a made-up config folder instead of the user's
    if let Some(dir) = std::env::var_os("NATIVETERM_SECURECRT_CONFIG").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir));
    }
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
    let new_folders = plan.folders.iter().filter(|f| f.existing.is_none() && f.session_count() > 0).count();
    let existing = plan.folders.iter().filter(|f| f.existing.is_some() && f.session_count() > 0).count();
    out.push(line(
        t!(
            "summary-found",
            sessions = scan.sessions.len(),
            folders = scan.folders.len(),
            hosts = plan.host_count(),
            new = new_folders,
            existing = existing
        ),
        false,
        plan.folders
            .iter()
            .filter(|f| f.session_count() > 0)
            .map(|f| format!("{} ({})", f.label, f.session_count()))
            .collect(),
    ));

    let mut skipped: BTreeMap<(bool, String), Vec<String>> = BTreeMap::new();
    for (path, why) in &plan.skipped {
        let reason = match why {
            Skip::AlreadyImported { .. } => (false, t!("skip-already")),
            Skip::Protocol(p) => (true, t!("skip-protocol", protocol = p.as_str())),
            Skip::NoHostname => (true, t!("skip-no-hostname")),
        };
        skipped.entry(reason).or_default().push(path.clone());
    }
    for ((warning, reason), paths) in skipped {
        out.push(line(t!("summary-skipped", reason = reason, count = paths.len()), warning, paths));
    }

    let n = &plan.notes;
    if !n.duplicates.is_empty() {
        out.push(line(
            t!("summary-duplicates", count = n.duplicates.len()),
            true,
            n.duplicates.iter().map(|g| g.join("  =  ")).collect(),
        ));
    }
    for (name, paths) in &n.named_firewalls {
        out.push(line(t!("summary-firewall", name = name.as_str(), count = paths.len()), true, paths.clone()));
    }
    if !n.unresolved_jumps.is_empty() {
        out.push(line(
            t!("summary-unresolved-jumps", count = n.unresolved_jumps.len()),
            true,
            n.unresolved_jumps.iter().map(|(p, t)| format!("{p}  →  {t}")).collect(),
        ));
    }
    if !n.logon_actions.is_empty() {
        out.push(line(t!("summary-logon-actions", count = n.logon_actions.len()), true, n.logon_actions.clone()));
    }
    if n.saved_passwords > 0 {
        out.push(line(t!("summary-saved-passwords", count = n.saved_passwords), true, Vec::new()));
    }
    if !n.encodings.is_empty() {
        out.push(line(
            t!("summary-encodings", count = n.encodings.len()),
            true,
            n.encodings.iter().map(|(p, e)| format!("{p}  ({e})")).collect(),
        ));
    }
    if !n.bad_forwards.is_empty() {
        out.push(line(t!("summary-bad-forwards", count = n.bad_forwards.len()), true, n.bad_forwards.clone()));
    }
    let mut kept = Vec::new();
    if n.forwards > 0 {
        kept.push(t!("summary-forwards", count = n.forwards));
    }
    if n.identity_files > 0 {
        kept.push(t!("summary-keys", count = n.identity_files));
    }
    if !n.ppk_keys.is_empty() {
        out.push(line(
            t!("summary-ppk", count = n.ppk_keys.len()),
            true,
            n.ppk_keys.iter().map(|(p, k)| format!("{p}  ({k})")).collect(),
        ));
    }
    if n.joined_descriptions > 0 {
        kept.push(t!("summary-descriptions", count = n.joined_descriptions));
    }
    if !kept.is_empty() {
        out.push(line(t!("summary-also", items = kept.join(", ")), false, Vec::new()));
    }
    if !scan.unreadable.is_empty() {
        out.push(line(
            t!("summary-unreadable", count = scan.unreadable.len()),
            true,
            scan.unreadable.iter().map(|(p, e)| format!("{p}: {e}")).collect(),
        ));
    }
    if !scan.not_utf8.is_empty() {
        out.push(line(t!("summary-not-utf8", count = scan.not_utf8.len()), true, scan.not_utf8.clone()));
    }
    let keys = &scan.host_keys;
    if !keys.keys.is_empty() {
        out.push(line(t!("summary-host-keys", count = keys.keys.len()), false, Vec::new()));
    }
    if keys.not_understood > 0 {
        let text = match scan.origin {
            Origin::SecureCrt => t!("summary-host-keys-unknown", count = keys.not_understood),
            Origin::Putty => t!("summary-putty-host-keys-skipped", count = keys.not_understood),
        };
        out.push(line(text, true, Vec::new()));
    }
    if scan.origin == Origin::SecureCrt {
        out.push(line(t!("summary-not-yet"), false, Vec::new()));
    }
    out
}
