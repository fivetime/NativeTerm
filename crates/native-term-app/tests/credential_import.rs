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
