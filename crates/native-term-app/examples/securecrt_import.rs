//! Preview (and optionally run) a SecureCRT import from the command line.
//!
//! ```text
//! cargo run --release -p native-term-app --example securecrt_import -- [options]
//!   --config <dir>    SecureCRT's config folder (default: its Config Path)
//!   --details         list session names under each line
//!   --write --ssh-dir <dir>   import into that ssh folder
//! ```
//!
//! Without `--write` nothing is written. The summary prints counts and
//! folder names; session names only with `--details`.

use std::path::PathBuf;

use native_term_app::import;
use native_term_config::ops::Editor;
use native_term_config::securecrt;
use native_term_config::write::Writer;
use native_term_config::SessionTree;

fn main() {
    let mut config = None;
    let mut ssh_dir: Option<PathBuf> = None;
    let mut details = false;
    let mut write = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => config = args.next().map(PathBuf::from),
            "--ssh-dir" => ssh_dir = args.next().map(PathBuf::from),
            "--details" => details = true,
            "--write" => write = true,
            other => {
                eprintln!("unknown argument {other}");
                std::process::exit(2);
            }
        }
    }
    let Some(config) = config.or_else(import::securecrt_config_path) else {
        eprintln!("SecureCRT's config folder wasn't found; pass --config");
        std::process::exit(1);
    };
    let home_ssh = std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join(".ssh"));
    let target = ssh_dir.clone().or(home_ssh.clone()).expect("USERPROFILE");
    let scan = match securecrt::scan(&config) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{}: {e}", config.display());
            std::process::exit(1);
        }
    };
    let tree = SessionTree::load(&target);
    let plan = securecrt::plan(&scan, &tree);
    println!("SecureCRT: {}", config.display());
    println!("into:      {}", target.display());
    for l in import::summary(&scan, &plan) {
        println!("{} {}", if l.warning { "!" } else { "-" }, l.text);
        let folders_line = l.text.contains("will be imported");
        if details || folders_line {
            for d in &l.details {
                println!("      {d}");
            }
        }
    }
    if !write {
        println!("\nPreview only; nothing was written.");
        return;
    }
    let Some(ssh_dir) = ssh_dir else {
        eprintln!("--write needs --ssh-dir (pass your .ssh folder explicitly)");
        std::process::exit(2);
    };
    let data = native_term_app::data_dir::Inputs::from_system(None)
        .and_then(|inputs| native_term_app::data_dir::resolve(&inputs).map(|(dir, _)| dir))
        .unwrap_or_else(|_| std::env::temp_dir().join("NativeTerm"));
    println!("backups:   {}", data.join("backups").display());
    let writer = Writer::new(data.join("backups"));
    let program = native_term_session::ssh_program();
    let ssh = program.as_path();
    println!("ssh:       {}", ssh.display());
    let is_home = home_ssh.as_deref().is_some_and(|h| h.to_string_lossy().eq_ignore_ascii_case(&ssh_dir.to_string_lossy()));
    let editor = if is_home { Editor::new(&ssh_dir, writer, ssh) } else { Editor::for_directory(&ssh_dir, writer, ssh) };
    let started = std::time::Instant::now();
    match editor.import(&plan, &|done, total| eprint!("\r{done}/{total}")) {
        Ok(outcome) => {
            eprintln!();
            println!("imported {} hosts into {} folders in {:.1} s", outcome.hosts(), outcome.written.len(), started.elapsed().as_secs_f64());
            println!("host keys added to known_hosts: {}", outcome.keys_added);
            if let Some(e) = &outcome.keys_failed {
                println!("! known_hosts not changed: {e}");
            }
            for (label, why) in &outcome.failed {
                println!("! folder {label} not written: {why}");
            }
        }
        Err(e) => {
            eprintln!("import failed: {e}");
            std::process::exit(1);
        }
    }
}
