//! Running the local command a host asks for before connecting
//! (`NativeTermPreConnect`, or a `.nt.toml` session's `pre_connect`).
//!
//! It runs in the tab, so what it prints is what the person sees, and the
//! connection waits for it — that is the point: the tunnel has to be up
//! before ssh starts. It is run again before every attempt, so a
//! reconnect after sleep brings the VPN back up too.
//!
//! A command that fails is said so and the connection goes ahead, because
//! plenty of useful pre-connect commands "fail" (a VPN that was already
//! up, a `ping` that warms an ARP entry). A command written with `!` in
//! front stops the connection instead.

use std::process::Command;

use native_term_config::preconnect;
use native_term_config::tree::SessionTree;

use crate::{plink, t};

/// What the command left behind.
pub enum Ran {
    /// Nothing to run, or it ran and the connection goes on.
    Go,
    /// It failed and the command said that stops the connection.
    Stop,
}

/// The host's pre-connect command (its own, else its folder's).
pub fn for_alias(alias: &str) -> Option<String> {
    let tree = SessionTree::load(&plink::ssh_dir());
    let (folder, host) = tree.find(alias)?;
    preconnect::for_host(folder, host)
}

/// Run `command` and say whether the connection should go on. The shell
/// is `cmd /c`, as everywhere else a user types a Windows command line.
pub fn run(command: &str) -> Ran {
    let (stop_on_failure, command) = preconnect::stop_on_failure(command);
    println!("{}", t!("preconnect-running", command = command));
    let comspec = std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".to_string());
    let status = Command::new(comspec).arg("/c").arg(command).status();
    match status {
        Ok(status) if status.success() => Ran::Go,
        Ok(status) => {
            let code = status.code().unwrap_or(-1);
            match stop_on_failure {
                true => {
                    println!("{}", t!("preconnect-stopped", code = code));
                    Ran::Stop
                }
                false => {
                    println!("{}", t!("preconnect-failed", code = code));
                    Ran::Go
                }
            }
        }
        Err(e) => {
            println!("{}", t!("preconnect-not-started", error = e.to_string()));
            match stop_on_failure {
                true => Ran::Stop,
                false => Ran::Go,
            }
        }
    }
}
