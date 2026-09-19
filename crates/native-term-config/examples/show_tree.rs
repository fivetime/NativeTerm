//! Print the session tree of an ssh directory (read-only).
//!
//! cargo run -p native-term-config --example show_tree [-- <ssh-dir>]

use std::path::PathBuf;
use std::time::Instant;

use native_term_config::{effective::effective, SessionTree};

fn main() {
    let ssh_dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").expect("USERPROFILE")).join(".ssh"));
    let started = Instant::now();
    let tree = SessionTree::load(&ssh_dir);
    println!("{} parsed in {:?}", ssh_dir.display(), started.elapsed());

    for folder in tree.folders() {
        let title = if folder.name.is_empty() { "(~/.ssh/config)" } else { folder.label() };
        println!("\n[{title}] {}", folder.file.display());
        for host in &folder.hosts {
            let extra = if host.aliases.len() > 1 {
                format!(" (also {})", host.aliases[1..].join(", "))
            } else {
                String::new()
            };
            println!(
                "  {:<28} -> {}{}{}{}  line {}",
                host.label(),
                host.user.as_deref().map(|u| format!("{u}@")).unwrap_or_default(),
                host.target(),
                host.port.map(|p| format!(":{p}")).unwrap_or_default(),
                extra,
                host.line + 1
            );
        }
    }
    for shared in &tree.shared {
        println!(
            "shared settings: Host {}  ({} line {})",
            shared.patterns.join(" "),
            shared.file.display(),
            shared.line + 1
        );
    }
    for warning in &tree.warnings {
        println!("warning: {warning:?}");
    }
    let first = tree.hosts().next().map(|(_, h)| h);
    if let Some(host) = first {
        let started = Instant::now();
        match effective(std::path::Path::new("ssh"), host.alias()) {
            Ok(pairs) => {
                let pick = |k: &str| pairs.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str()).unwrap_or("-");
                println!(
                    "\nssh -G {} ({:?}): hostname {} user {} port {}",
                    host.alias(),
                    started.elapsed(),
                    pick("hostname"),
                    pick("user"),
                    pick("port")
                );
            }
            Err(e) => println!("\nssh -G {} failed: {e}", host.alias()),
        }
    }
}
