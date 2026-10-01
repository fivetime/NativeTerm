//! Importing from SecureCRT or PuTTY: where the sessions are, and the
//! summary shown before anything is written.

use std::collections::BTreeMap;
use std::path::PathBuf;

use native_term_config::securecrt::{LogonSecret, Origin, Plan, PlannedCredential, Scan, Skip};

use crate::t;

/// SecureCRT's configuration folder (`HKCU\Software\VanDyke\SecureCRT`,
/// `Config Path`), if it is installed and the folder exists.
pub fn securecrt_config_path() -> Option<PathBuf> {
    // tests: a made-up config folder instead of the user's
    if let Some(dir) = std::env::var_os("NATIVETERM_SECURECRT_CONFIG").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    #[cfg(windows)]
    {
        let value =
            native_term_os::desktop::user_registry_string(r"Software\VanDyke\SecureCRT", "Config Path").ok()??;
        let path = PathBuf::from(value);
        path.join("Sessions").is_dir().then_some(path)
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// SecureCRT's saved credentials kept as credential sets, after the
/// configuration was written (its hosts name them): the password read from
/// SecureCRT's file and written straight into the system's password store,
/// the user name with it. A set of that name that is there already is left
/// as it is. What happened, a line each.
pub fn store_credentials(credentials: &[PlannedCredential]) -> Vec<String> {
    use native_term_config::password;
    use native_term_os::credentials::{self, Saved};
    let mut out = Vec::new();
    let mut added = 0;
    for c in credentials {
        let entry = password::set_entry(&c.set);
        if credentials::read(&entry).ok().flatten().is_some() {
            out.push(t!("import-credential-kept", set = c.set.as_str()));
            continue;
        }
        let secret = match native_term_config::securecrt::read_credential_password(&c.file, "") {
            Ok(Some(secret)) => secret,
            Ok(None) => {
                out.push(t!("import-credential-no-password", title = c.title.as_str()));
                continue;
            }
            Err(_) => {
                out.push(t!("import-credential-unread", title = c.title.as_str()));
                continue;
            }
        };
        let saved = Saved { user: c.user.clone().unwrap_or_default(), secret, comment: String::new() };
        match credentials::write(&entry, &saved) {
            Ok(()) => added += 1,
            Err(e) => out.push(t!("import-credential-failed", set = c.set.as_str(), error = e.to_string())),
        }
        drop(saved);
    }
    if added > 0 {
        out.insert(0, t!("import-credentials-added", count = added));
    }
    out
}

/// Hidden logon action Sends, after the configuration was written (its
/// rows say `secret:<id>`): each read from its SecureCRT session file and
/// written straight into the system's password store, as the Logon Actions
/// page keeps a hidden row's. What went wrong, a line each, after how many
/// were kept.
pub fn store_logon_secrets(secrets: &[LogonSecret]) -> Vec<String> {
    use native_term_config::logon;
    use native_term_os::credentials::{self, Saved};
    let mut out = Vec::new();
    let mut added = 0;
    for s in secrets {
        let row = s.row + 1;
        let secret = match native_term_config::securecrt::read_logon_send(&s.file, s.row, "") {
            Ok(Some(secret)) => secret,
            Ok(None) | Err(_) => {
                out.push(t!("import-logon-unread", session = s.session.as_str(), row = row));
                continue;
            }
        };
        let saved = Saved { user: String::new(), secret, comment: String::new() };
        match credentials::write(&logon::secret_entry(&s.id), &saved) {
            Ok(()) => added += 1,
            Err(e) => {
                out.push(t!("import-logon-failed", session = s.session.as_str(), row = row, error = e.to_string()))
            }
        }
        drop(saved);
    }
    if added > 0 {
        out.insert(0, t!("import-logon-added", count = added));
    }
    out
}

/// The imported hosts' own saved passwords, after the configuration was
/// written: each read from its SecureCRT session file and written straight
/// into the system's password store as the account's entry, named as the
/// shim looks it up at connecting (`password::target` of `ssh -G`). An
/// entry already there is left as it is. What happened, in a few lines.
pub fn store_session_passwords(editor: &native_term_config::ops::Editor, plan: &Plan) -> Vec<String> {
    use native_term_config::password;
    use native_term_os::credentials::{self, Saved};
    let (mut added, mut kept, mut unread, mut no_account) = (0, 0, 0, 0);
    let mut failed = Vec::new();
    for host in plan.folders.iter().flat_map(|f| &f.hosts) {
        let Some(file) = &host.password_file else { continue };
        let target = editor.effective(&host.alias).ok().and_then(|e| password::target(&e, None));
        let Some(target) = target else {
            no_account += 1;
            continue;
        };
        if credentials::read(&target.name).ok().flatten().is_some() {
            kept += 1;
            continue;
        }
        let secret = match native_term_config::securecrt::read_credential_password(file, "") {
            Ok(Some(secret)) => secret,
            Ok(None) => continue,
            Err(_) => {
                unread += 1;
                continue;
            }
        };
        let saved = Saved { user: target.user.clone(), secret, comment: String::new() };
        match credentials::write(&target.name, &saved) {
            Ok(()) => added += 1,
            Err(e) => failed.push(t!("import-password-failed", label = host.label.as_str(), error = e.to_string())),
        }
        drop(saved);
    }
    let mut out = Vec::new();
    if added > 0 {
        out.push(t!("import-passwords-added", count = added));
    }
    if kept > 0 {
        out.push(t!("import-passwords-kept", count = kept));
    }
    if unread > 0 {
        out.push(t!("import-passwords-unread", count = unread));
    }
    if no_account > 0 {
        out.push(t!("import-passwords-no-account", count = no_account));
    }
    out.extend(failed);
    out
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
        out.push(line(t!("summary-logon-actions", count = n.logon_actions.len()), false, n.logon_actions.clone()));
    }
    if !plan.logon_secrets.is_empty() {
        out.push(line(t!("summary-logon-hidden", count = plan.logon_secrets.len()), false, Vec::new()));
    }
    if !n.logon_unread.is_empty() {
        out.push(line(t!("summary-logon-unread", count = n.logon_unread.len()), true, n.logon_unread.clone()));
    }
    let with_password = plan.folders.iter().flat_map(|f| &f.hosts).filter(|h| h.password_file.is_some()).count();
    if with_password > 0 {
        out.push(line(t!("summary-session-passwords", count = with_password), false, Vec::new()));
    }
    let left_out = n.saved_passwords.saturating_sub(with_password);
    if left_out > 0 {
        out.push(line(t!("summary-saved-passwords", count = left_out), true, Vec::new()));
    }
    if !plan.credentials.is_empty() {
        out.push(line(
            t!("summary-credentials", count = plan.credentials.len()),
            false,
            plan.credentials
                .iter()
                .map(|c| {
                    let user = c.user.as_deref().unwrap_or("-");
                    t!("summary-credential", set = c.set.as_str(), user = user, sessions = c.sessions)
                })
                .collect(),
        ));
    }
    if !n.missing_credentials.is_empty() {
        out.push(line(
            t!("summary-missing-credentials", count = n.missing_credentials.len()),
            true,
            n.missing_credentials.iter().map(|(p, t)| format!("{p}  ({t})")).collect(),
        ));
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
