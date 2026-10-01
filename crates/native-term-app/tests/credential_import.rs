#![cfg(windows)]
//! SecureCRT's saved credentials into credential sets: a made-up
//! configuration (a password encrypted the way SecureCRT 9 does, made with
//! the public decoder), stored under a prefix of the test's own
//! (`NATIVETERM_CRED_PREFIX`; a binary of its own, so no other test sees
//! it), read back, and removed.

use std::path::Path;

use native_term_config::securecrt;
use native_term_config::tree::SessionTree;
use native_term_os::credentials;

const S3CRET: &str = "03:ee37ed18fd80d738b04abd10e247b4a90a83c120f1c2bce15411fec48149add6596d7f7a7a7e94a29d1bee9e229c11744d879c4becb1e7c1b9aa1c3140b08e03e3a08a9246d892ea30affc8a9f600b1f";

fn write(path: &Path, lines: &[&str]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, format!("\u{feff}{}\r\n", lines.join("\r\n"))).unwrap();
}

#[test]
fn saved_credentials_land_in_the_password_store() {
    let prefix = format!("NativeTerm-Tests-crtimport-{}", std::process::id());
    std::env::set_var("NATIVETERM_CRED_PREFIX", &prefix);
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir.path().join("Credentials").join("login.ini"),
        &["S:\"Username\"=root", &format!("S:\"Password V2\"={S3CRET}")],
    );
    write(&dir.path().join("Credentials").join("empty.ini"), &["S:\"Username\"=nobody", "S:\"Password V2\"="]);
    write(&dir.path().join("Credentials").join("damaged.ini"), &["S:\"Username\"=x", "S:\"Password V2\"=03:00112233"]);
    write(&dir.path().join("Sessions").join("web.ini"), &["S:\"Hostname\"=10.0.0.1", "S:\"Credential Title\"=login"]);
    let scan = securecrt::scan(dir.path()).unwrap();
    let plan = securecrt::plan(&scan, &SessionTree::default());
    let entry = native_term_config::password::set_entry("login");
    let _ = credentials::delete(&entry);

    let lines = native_term_app::import::store_credentials(&plan.credentials);
    let stored = credentials::read(&entry).unwrap();
    let again = native_term_app::import::store_credentials(&plan.credentials);
    let _ = credentials::delete(&entry);
    for set in ["empty", "damaged"] {
        let _ = credentials::delete(&native_term_config::password::set_entry(set));
    }

    let stored = stored.expect("the set is stored");
    assert_eq!((stored.user.as_str(), stored.secret.as_str()), ("root", "s3cret"));
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert!(lines[0].contains('1'), "one set made: {lines:?}");
    assert!(lines.iter().any(|l| l.contains("empty")), "no password: {lines:?}");
    assert!(lines.iter().any(|l| l.contains("damaged")), "not read: {lines:?}");
    assert!(again.iter().any(|l| l.contains("login")), "kept the second time: {again:?}");
    assert!(!lines.iter().chain(&again).any(|l| l.contains("s3cret")), "the password is never said");
}

/// A session's own saved password: imported with the session (OpenSSH
/// asked for the account, as the shim asks at connecting), then kept as
/// that account's entry; one already there is left as it is.
#[test]
fn a_sessions_saved_password_becomes_its_accounts() {
    use native_term_config::ops::Editor;
    use native_term_config::write::Writer;
    let ssh = std::path::PathBuf::from(r"C:\Windows\System32\OpenSSH\ssh.exe");
    if !ssh.exists() {
        eprintln!("skipped: no Windows OpenSSH");
        return;
    }
    let prefix = format!("NativeTerm-Tests-crtimport-{}", std::process::id());
    std::env::set_var("NATIVETERM_CRED_PREFIX", &prefix);
    let crt = tempfile::tempdir().unwrap();
    write(
        &crt.path().join("Sessions").join("lab").join("db.ini"),
        &[
            "S:\"Hostname\"=10.0.0.9",
            "S:\"Username\"=root",
            "D:\"[SSH2] Port\"=00000016",
            &format!("S:\"Password V2\"={S3CRET}"),
        ],
    );
    write(
        &crt.path().join("Sessions").join("lab").join("nouser.ini"),
        &["S:\"Hostname\"=10.0.0.8", &format!("S:\"Password V2\"={S3CRET}")],
    );
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join(".ssh");
    std::fs::create_dir_all(&dir).unwrap();
    let editor = Editor::for_directory(&dir, Writer::new(home.path().join("backups")), &ssh);
    std::fs::write(
        editor.main_config(),
        "# mine
",
    )
    .unwrap();
    let scan = securecrt::scan(crt.path()).unwrap();
    let plan = securecrt::plan(&scan, &SessionTree::load(&dir));
    editor.import(&plan, &|_, _| {}).unwrap();
    let entry = format!("{prefix}:root@10.0.0.9:22");
    let _ = credentials::delete(&entry);

    let lines = native_term_app::import::store_session_passwords(&editor, &plan);
    let stored = credentials::read(&entry).unwrap();
    // the session without a user name: ssh's own, this computer's user
    let local = credentials::list(&format!("{prefix}:")).unwrap();
    let again = native_term_app::import::store_session_passwords(&editor, &plan);
    for name in &local {
        let _ = credentials::delete(name);
    }

    let stored = stored.expect("the account's entry");
    assert_eq!((stored.user.as_str(), stored.secret.as_str()), ("root", "s3cret"));
    assert_eq!(local.len(), 2, "both accounts, as ssh names them: {local:?}");
    assert!(local.iter().any(|n| n.ends_with("@10.0.0.8:22") && !n.contains(":root@")), "{local:?}");
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains('2'), "two stored: {lines:?}");
    assert!(again.len() == 1 && again[0].contains('2'), "kept the second time: {again:?}");
    assert!(!lines.iter().chain(&again).any(|l| l.contains("s3cret")));
}
