//! Docking on KDE Plasma under Wayland, where no program may place its own
//! window or see the pointer outside it: KWin itself does it, through a
//! script NativeTerm loads into it (`kwin_dock.js`, over D-Bus with
//! `dbus-send`) and unloads when it is done. The script finds the window
//! by NativeTerm's process id.

use std::io;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const SCRIPT: &str = include_str!("kwin_dock.js");

/// Whether this is a KDE Plasma session under Wayland.
#[must_use]
pub fn available() -> bool {
    let kde = std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.split(':').any(|p| p.eq_ignore_ascii_case("KDE")));
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some_and(|d| !d.is_empty());
    kde && wayland
}

/// The script as loaded for process `pid`.
fn script_for(pid: u32, log: bool) -> String {
    SCRIPT
        .replace("__NATIVETERM_PID__", &pid.to_string())
        .replace("__NATIVETERM_LOG__", if log { "true" } else { "false" })
}

/// What `dbus-send --print-reply` printed, when it succeeded.
fn dbus(path: &str, method: &str, args: &[String]) -> io::Result<String> {
    let out = Command::new("dbus-send")
        .args(["--session", "--print-reply", "--dest=org.kde.KWin", path, method])
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(io::Error::other(String::from_utf8_lossy(&out.stderr).trim().to_string()))
    }
}

/// Unload the scripts, and remove the files, of NativeTerms no longer
/// running (and one of this pid, from before).
fn forget_stale(dir: &std::path::Path, pid: u32) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let file = entry.file_name().to_string_lossy().into_owned();
        let Some(other) = file.strip_prefix("nativeterm-dock-").and_then(|f| f.strip_suffix(".js")) else {
            continue;
        };
        let Ok(other) = other.parse::<u32>() else { continue };
        if other == pid || !std::path::Path::new(&format!("/proc/{other}")).exists() {
            let _ =
                dbus("/Scripting", "org.kde.kwin.Scripting.unloadScript", &[format!("string:nativeterm-dock-{other}")]);
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// The script, running in KWin while this lives.
pub struct KwinDock {
    name: String,
    file: PathBuf,
}

impl KwinDock {
    /// Load and run the docking script for this process; `log` has it
    /// print what it does (to KWin's log, `journalctl --user`).
    pub fn start(log: bool) -> io::Result<KwinDock> {
        let pid = std::process::id();
        let name = format!("nativeterm-dock-{pid}");
        let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let file = dir.join(format!("{name}.js"));
        // ones left by NativeTerms that did not exit cleanly (killed): their
        // process is gone (before this one's file is written)
        forget_stale(&dir, pid);
        std::fs::write(&file, script_for(pid, log))?;
        let reply = dbus(
            "/Scripting",
            "org.kde.kwin.Scripting.loadScript",
            &[format!("string:{}", file.display()), format!("string:{name}")],
        )?;
        let id = reply
            .split_whitespace()
            .skip_while(|w| *w != "int32")
            .nth(1)
            .and_then(|n| n.parse::<i32>().ok())
            .filter(|n| *n >= 0)
            .ok_or_else(|| io::Error::other(format!("KWin did not load the script: {}", reply.trim())))?;
        // KWin 6 names the script's object /Scripting/Script<id>, KWin 5 /<id>
        dbus(&format!("/Scripting/Script{id}"), "org.kde.kwin.Script.run", &[])
            .or_else(|_| dbus(&format!("/{id}"), "org.kde.kwin.Script.run", &[]))?;
        Ok(KwinDock { name, file })
    }
}

impl Drop for KwinDock {
    fn drop(&mut self) {
        let _ = dbus("/Scripting", "org.kde.kwin.Scripting.unloadScript", &[format!("string:{}", self.name)]);
        let _ = std::fs::remove_file(&self.file);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_script_gets_the_pid() {
        let script = super::script_for(4242, false);
        assert!(script.contains("const PID = 4242;"));
        assert!(!script.contains("__NATIVETERM"));
        assert!(script.contains("if (false) print("));
    }
}
