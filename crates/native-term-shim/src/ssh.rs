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
pub fn arguments(
    alias: &str,
    shim_exe: &Path,
    shim_pid: u32,
    effective: &[(String, String)],
    no_forwards: bool,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = Vec::new();
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
    args.push("--".into());
    args.push(alias.into());
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[OsString]) -> Vec<String> {
        args.iter().map(|a| a.to_string_lossy().to_string()).collect()
    }

    #[test]
    fn full_command_line() {
        let args = arguments("web01", Path::new(r"C:\Program Files\NativeTerm\nativeterm-shim.exe"), 77, &[], false);
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
        let args = strings(&arguments("web01", Path::new(r"C:\nt\nativeterm-shim.exe"), 1, &effective, false));
        assert!(!args.iter().any(|a| a.starts_with("ServerAlive")), "{args:?}");
    }

    #[test]
    fn a_clone_drops_forwards() {
        let args = strings(&arguments("web01", Path::new(r"C:\nt\nativeterm-shim.exe"), 1, &[], true));
        let at = args.iter().position(|a| a == "ClearAllForwardings=yes").expect("option");
        assert_eq!(args[at - 1], "-o");
        assert!(at < args.iter().position(|a| a == "--").unwrap());
    }

    #[test]
    fn percent_in_path_drops_the_login_signal() {
        let args = strings(&arguments("web01", Path::new(r"C:\100%\nativeterm-shim.exe"), 1, &[], false));
        assert!(!args.iter().any(|a| a.contains("LocalCommand")), "{args:?}");
        assert_eq!(args.last().unwrap(), "web01");
    }
}
