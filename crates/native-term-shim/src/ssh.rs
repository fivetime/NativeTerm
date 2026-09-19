//! The ssh command line, built by the shim itself so nothing complex ever
//! passes through `wt`.

use std::ffi::OsString;
use std::path::Path;

pub const KEEPALIVE_INTERVAL: u32 = 15;
pub const KEEPALIVE_COUNT: u32 = 3;

/// Arguments for `ssh`, ending with `-- <alias>`.
///
/// - Login signal: `LocalCommand` runs through `cmd.exe /c` after
///   authentication. The shim's path is quoted for `cmd.exe`; a path with
///   `%` can't be passed safely (both ssh and `cmd.exe` expand it), so the
///   signal is left out then.
/// - Keepalives only when `effective` (from `ssh -G`) shows the user hasn't
///   set them; command-line options would override the config.
/// - `config`: `-F <file>` for a folder other than `~/.ssh` (`--ssh-dir`).
/// - `remote`: a `RemoteCommand` (a persistent session), with a terminal:
///   ssh only asks for one by itself when there is no command.
pub fn arguments(
    alias: &str,
    shim_exe: &Path,
    shim_pid: u32,
    effective: &[(String, String)],
    no_forwards: bool,
    config: Option<&Path>,
    remote: Option<&str>,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = Vec::new();
    if let Some(config) = config {
        args.push("-F".into());
        args.push(config.into());
    }
    let mut option = |value: String| {
        args.push("-o".into());
        args.push(value.into());
    };
    let exe = shim_exe.to_string_lossy();
    if !exe.contains('%') && !exe.contains('"') {
        option("PermitLocalCommand=yes".into());
        option(format!("LocalCommand=\"{exe}\" --authenticated {shim_pid}"));
    }
    let value = |key: &str| effective.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
    if value("serveraliveinterval").unwrap_or("0") == "0" {
        option(format!("ServerAliveInterval={KEEPALIVE_INTERVAL}"));
        if value("serveralivecountmax").unwrap_or("3") == "3" {
            option(format!("ServerAliveCountMax={KEEPALIVE_COUNT}"));
        }
    }
    if no_forwards {
        // a clone: the original holds the forwarded ports
        option("ClearAllForwardings=yes".into());
    }
    if let Some(remote) = remote {
        option("RequestTTY=yes".into());
        option(format!("RemoteCommand={remote}"));
    }
    args.push("--".into());
    args.push(alias.into());
    args
}

/// Whether ssh connects to the host itself: no `ProxyCommand` or
/// `ProxyJump` in `effective` (from `ssh -G`). Only then do ssh's own TCP
/// connections show whether the server was reached. Unknown (no `ssh -G`
/// output) counts as not direct.
pub fn is_direct(effective: &[(String, String)]) -> bool {
    let set = |key: &str| effective.iter().any(|(k, v)| k == key && !v.eq_ignore_ascii_case("none"));
    !effective.is_empty() && !set("proxycommand") && !set("proxyjump")
}

/// Watches ssh's TCP connections until one is established.
pub struct Reached {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    reached: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Reached {
    const EVERY: std::time::Duration = std::time::Duration::from_millis(250);

    pub fn watch(pid: u32) -> Reached {
        use std::sync::atomic::Ordering;
        let watch = Reached { stop: Default::default(), reached: Default::default() };
        let (stop, reached) = (watch.stop.clone(), watch.reached.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                if crate::win::tcp_states(pid).contains(&crate::win::TCP_ESTABLISHED) {
                    reached.store(true, Ordering::Relaxed);
                    return;
                }
                std::thread::sleep(Self::EVERY);
            }
        });
        watch
    }

    /// Stops watching; whether a connection was established.
    pub fn stop(self) -> bool {
        use std::sync::atomic::Ordering;
        self.stop.store(true, Ordering::Relaxed);
        self.reached.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_only_without_a_proxy() {
        let pair = |k: &str, v: &str| (k.to_string(), v.to_string());
        assert!(is_direct(&[pair("hostname", "10.0.0.1")]));
        assert!(is_direct(&[pair("hostname", "10.0.0.1"), pair("proxycommand", "none")]));
        assert!(!is_direct(&[pair("hostname", "10.0.0.1"), pair("proxyjump", "bastion")]));
        assert!(!is_direct(&[pair("proxycommand", "ssh -W %h:%p bastion")]));
        assert!(!is_direct(&[]), "unknown");
    }

    fn strings(args: &[OsString]) -> Vec<String> {
        args.iter().map(|a| a.to_string_lossy().to_string()).collect()
    }

    #[test]
    fn full_command_line() {
        let args = arguments(
            "web01",
            Path::new(r"C:\Program Files\NativeTerm\nativeterm-shim.exe"),
            77,
            &[],
            false,
            None,
            None,
        );
        assert_eq!(
            strings(&args),
            vec![
                "-o",
                "PermitLocalCommand=yes",
                "-o",
                r#"LocalCommand="C:\Program Files\NativeTerm\nativeterm-shim.exe" --authenticated 77"#,
                "-o",
                "ServerAliveInterval=15",
                "-o",
                "ServerAliveCountMax=3",
                "--",
                "web01",
            ]
        );
    }

    #[test]
    fn user_keepalive_is_respected() {
        let effective = vec![("serveraliveinterval".to_string(), "60".to_string())];
        let args =
            strings(&arguments("web01", Path::new(r"C:\nt\nativeterm-shim.exe"), 1, &effective, false, None, None));
        assert!(!args.iter().any(|a| a.starts_with("ServerAlive")), "{args:?}");
    }

    #[test]
    fn a_clone_drops_forwards() {
        let args = strings(&arguments("web01", Path::new(r"C:\nt\nativeterm-shim.exe"), 1, &[], true, None, None));
        let at = args.iter().position(|a| a == "ClearAllForwardings=yes").expect("option");
        assert_eq!(args[at - 1], "-o");
        assert!(at < args.iter().position(|a| a == "--").unwrap());
    }

    /// `--ssh-dir`: ssh reads that folder's config, not `~/.ssh/config`.
    #[test]
    fn another_folder_is_passed_with_dash_f() {
        let config = Path::new(r"C:\nt-test\ssh\config");
        let args =
            strings(&arguments("web01", Path::new(r"C:\nt\nativeterm-shim.exe"), 1, &[], false, Some(config), None));
        assert_eq!(args[..2], ["-F", r"C:\nt-test\ssh\config"]);
        assert_eq!(args.last().unwrap(), "web01");
    }

    /// A persistent session: the command, with a terminal, before `--`.
    #[test]
    fn remote_command_with_a_terminal() {
        let args = strings(&arguments(
            "web01",
            Path::new(r"C:\nt\nativeterm-shim.exe"),
            1,
            &[],
            false,
            None,
            Some("sh -c 'x'"),
        ));
        let at = args.iter().position(|a| a == "RemoteCommand=sh -c 'x'").expect("command");
        assert_eq!(args[at - 2..at], ["RequestTTY=yes", "-o"]);
        assert_eq!(args[at + 1..], ["--", "web01"]);
    }

    #[test]
    fn percent_in_path_drops_the_login_signal() {
        let args = strings(&arguments("web01", Path::new(r"C:\100%\nativeterm-shim.exe"), 1, &[], false, None, None));
        assert!(!args.iter().any(|a| a.contains("LocalCommand")), "{args:?}");
        assert_eq!(args.last().unwrap(), "web01");
    }
}
