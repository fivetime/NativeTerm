//! Probe: with NativeTerm's writers taking turns (the store lock), do
//! processes that only read, at the same time, still lose or hide
//! entries? Workers write an entry, have a child process mark it (read,
//! change, write: `credentials::update`), then a fresh process reads it
//! and the worker reads it; meanwhile reader processes read other
//! entries as fast as they can. `cred_read_race [readers] [rounds]`.

#[cfg(windows)]
fn main() {
    use native_term_win::credentials::{self, Saved};
    use std::process::Command;

    let args: Vec<String> = std::env::args().collect();
    let me = std::env::current_exe().unwrap();
    match args.get(1).map(String::as_str) {
        Some("mark") => {
            let marked = credentials::update(&args[2], |s| s.comment = "marked".into());
            std::process::exit(match marked {
                Ok(true) => 0,
                Ok(false) => 3,
                Err(_) => 4,
            });
        }
        Some("check") => {
            let seen = credentials::read(&args[2]).map(|s| s.map(|s| s.comment));
            println!("{seen:?}");
        }
        Some("reader") => {
            let until = std::time::Instant::now() + std::time::Duration::from_secs(args[2].parse().unwrap());
            let name = format!("NativeTerm-Tests-readrace-reader-{}", std::process::id());
            let mut reads = 0u64;
            while std::time::Instant::now() < until {
                let _ = credentials::read(&name);
                let _ = credentials::read("NativeTerm-Tests-readrace-nothing");
                reads += 2;
            }
            println!("reader: {reads} reads");
        }
        Some("worker") => {
            let (id, rounds): (u32, u32) = (args[2].parse().unwrap(), args[3].parse().unwrap());
            let name = format!("NativeTerm-Tests-readrace-{}-{id}", std::process::id());
            let (mut missing, mut unmarked_here, mut unmarked_fresh, mut child_failed) = (0, 0, 0, 0);
            for _ in 0..rounds {
                let fresh = Saved { user: "u".into(), secret: "s".into(), comment: String::new() };
                credentials::write(&name, &fresh).unwrap();
                let status = Command::new(&me).args(["mark", &name]).status().unwrap();
                if !status.success() {
                    child_failed += 1;
                }
                let other = Command::new(&me).args(["check", &name]).output().unwrap();
                let other = String::from_utf8_lossy(&other.stdout).trim().to_string();
                if other != "Ok(Some(\"marked\"))" {
                    unmarked_fresh += 1;
                    eprintln!("worker {id}: a fresh process read {other}");
                }
                match credentials::read(&name).unwrap() {
                    None => missing += 1,
                    Some(s) if s.comment != "marked" => unmarked_here += 1,
                    Some(_) => {}
                }
            }
            let _ = credentials::delete(&name);
            println!(
                "worker {id}: missing {missing}, unmarked here {unmarked_here}, unmarked in a fresh process {unmarked_fresh}, mark failed {child_failed}"
            );
        }
        _ => {
            let readers: u32 = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(6);
            let rounds: u32 = args.get(2).and_then(|a| a.parse().ok()).unwrap_or(40);
            let seconds = (rounds / 2 + 10).to_string();
            let readers: Vec<_> =
                (0..readers).map(|_| Command::new(&me).args(["reader", &seconds]).spawn().unwrap()).collect();
            let workers: Vec<_> = (0..6)
                .map(|id| Command::new(&me).args(["worker", &id.to_string(), &rounds.to_string()]).spawn().unwrap())
                .collect();
            for mut w in workers.into_iter().chain(readers) {
                w.wait().unwrap();
            }
        }
    }
}

#[cfg(not(windows))]
fn main() {}
