//! Probe: is a Credential Manager write by one process visible to the
//! next process at once, while other processes write at the same time?
//! `cred_race` runs several workers; each, round after round, writes an
//! entry, has a child process mark it (read, change the comment, write)
//! and reads it back after the child exited.

#[cfg(windows)]
fn main() {
    use native_term_win::credentials::{self, Saved};
    use std::process::Command;

    let args: Vec<String> = std::env::args().collect();
    let me = std::env::current_exe().unwrap();
    match args.get(1).map(String::as_str) {
        Some("mark") => {
            let name = &args[2];
            match credentials::read(name) {
                Ok(Some(mut saved)) => {
                    saved.comment = "marked".into();
                    if let Err(e) = credentials::write(name, &saved) {
                        eprintln!("mark: write failed: {e}");
                        std::process::exit(2);
                    }
                }
                Ok(None) => {
                    eprintln!("mark: not found");
                    std::process::exit(3);
                }
                Err(e) => {
                    eprintln!("mark: read failed: {e}");
                    std::process::exit(4);
                }
            }
        }
        Some("check") => {
            let seen = credentials::read(&args[2]).map(|s| s.map(|s| s.comment));
            println!("{seen:?}");
        }
        Some("worker") => {
            let (id, rounds): (u32, u32) = (args[2].parse().unwrap(), args[3].parse().unwrap());
            let name = format!("NativeTerm-Tests-race-{}-{id}", std::process::id());
            let (mut lost, mut late, mut child_failed) = (0, 0, 0);
            for _ in 0..rounds {
                let fresh = Saved { user: "u".into(), secret: "s".into(), comment: String::new() };
                credentials::write(&name, &fresh).unwrap();
                let out = Command::new(&me).args(["mark", &name]).output().unwrap();
                if !out.status.success() {
                    child_failed += 1;
                    eprint!("{}", String::from_utf8_lossy(&out.stderr));
                }
                // another process's view first, then this one's
                let other = Command::new(&me).args(["check", &name]).output().unwrap();
                let other = String::from_utf8_lossy(&other.stdout).trim().to_string();
                let here = credentials::read(&name).unwrap().map(|s| s.comment);
                if here.as_deref() != Some("marked") {
                    lost += 1;
                    // later?
                    let mut when = None;
                    for ms in [20u64, 100, 500, 2000] {
                        std::thread::sleep(std::time::Duration::from_millis(ms));
                        if credentials::read(&name).unwrap().map(|s| s.comment).as_deref() == Some("marked") {
                            when = Some(ms);
                            break;
                        }
                    }
                    eprintln!("worker {id}: mark not seen at once; later: {when:?} (then {here:?})");
                } else if other != "Ok(Some(\"marked\"))" {
                    late += 1;
                }
            }
            let _ = credentials::delete(&name);
            println!("worker {id}: lost {lost}, seen late by another process {late}, child failed {child_failed}");
        }
        _ => {
            let workers: Vec<_> =
                (0..6).map(|id| Command::new(&me).args(["worker", &id.to_string(), "40"]).spawn().unwrap()).collect();
            for mut w in workers {
                w.wait().unwrap();
            }
        }
    }
}

#[cfg(not(windows))]
fn main() {}
