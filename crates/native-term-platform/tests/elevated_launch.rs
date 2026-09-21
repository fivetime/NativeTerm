//! The elevated path, from a process that really is elevated.
//!
//! An elevated `wt.exe` joins Terminal's *elevated* instance, not the
//! user's window, so `launch::run` asks the desktop shell to start it
//! instead: Explorer runs as the user, and the tab lands in the user's own
//! Terminal (see `launch.rs` and "Elevation decides which Terminal
//! instance a tab joins" in `docs/ARCHITECTURE.md`). Forcing that path
//! from a normal process (`portable_terminal::batches_through_the_shell`)
//! only shows that the code runs; whether it crosses the integrity
//! boundary can only be seen from above it.
//!
//! Only the portable test Terminal is touched, and only tabs this test
//! opened are closed.
//!
//! ```text
//! set NATIVETERM_TEST_WT_DIR=C:\…\terminal-1.26.2581.0
//! cargo test -p native-term-platform --test elevated_launch --no-run
//! :: then, from an elevated prompt, run the test binary it built:
//! <target\debug\deps\elevated_launch-….exe> --ignored --nocapture
//! ```

use std::ffi::OsString;
use std::time::{Duration, Instant};

use native_term_platform::windows_terminal::install::{Install, Kind};
use native_term_platform::windows_terminal::{launch, Mismatch, WindowsTerminal};
use native_term_win::Standing;

const WAIT: Duration = Duration::from_secs(20);

fn terminal() -> WindowsTerminal {
    let dir = std::env::var("NATIVETERM_TEST_WT_DIR").expect("set NATIVETERM_TEST_WT_DIR to a portable Terminal");
    let install = Install::from_dir(dir.as_ref()).unwrap();
    assert_eq!(install.kind, Kind::Portable, "only a portable Terminal is used for tests");
    WindowsTerminal::new(install, std::path::Path::new("nativeterm-shim.exe"))
}

/// A window of this Terminal that wasn't there before, and its process.
fn new_window(wt: &WindowsTerminal, before: &[isize]) -> Option<(isize, u32)> {
    let started = Instant::now();
    loop {
        if let Some(w) = wt.windows().into_iter().find(|w| !before.contains(&w.handle)) {
            return Some((w.handle, w.pid));
        }
        if started.elapsed() >= WAIT {
            return None;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn handles(wt: &WindowsTerminal) -> Vec<isize> {
    wt.windows().iter().map(|w| w.handle).collect()
}

/// Closes a window this test opened, and waits for it to go.
fn close(wt: &WindowsTerminal, window: isize) {
    native_term_win::desktop::close_window(window);
    let deadline = Instant::now() + WAIT;
    while handles(wt).contains(&window) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn tab(title: &str) -> Vec<OsString> {
    vec!["new-tab".into(), format!("--title={title}").into(), "--suppressApplicationTitle".into(), "cmd".into()]
}

#[test]
#[ignore = "needs a portable Windows Terminal and an elevated process"]
fn an_elevated_nativeterm_opens_tabs_in_the_users_terminal() {
    assert!(native_term_win::is_elevated(), "run this test from an elevated prompt (see the file header)");
    let wt = terminal();
    assert_eq!(wt.mismatch(), Some(Mismatch::WeAreElevated), "elevated: NativeTerm says so at start");

    // through the shell (what NativeTerm does when elevated)
    let before = handles(&wt);
    let mut args: Vec<OsString> = vec!["-w".into(), "new".into()];
    args.extend(tab("nt-elevated"));
    launch::run(&wt.install().launcher, &args).expect("launch through the shell");
    let (user_window, pid) = new_window(&wt, &before).expect("a window of the test Terminal appeared");
    assert!(wt.wait_for_titles(&["nt-elevated"], WAIT), "the tab is in it");

    // the point of all this: it belongs to the user, not to us
    assert_eq!(
        native_term_win::process_standing(pid),
        Standing::Normal,
        "the tab went to an elevated Terminal instead of the user's"
    );

    // and the other way round: started directly, it would be ours
    let before = handles(&wt);
    let mut args: Vec<OsString> = vec!["-w".into(), "new".into()];
    args.extend(tab("nt-elevated admin"));
    std::process::Command::new(&wt.install().launcher).args(&args).spawn().expect("wt").wait().ok();
    let (admin_window, admin_pid) = new_window(&wt, &before).expect("an elevated window appeared");
    assert_eq!(
        native_term_win::process_standing(admin_pid),
        Standing::Elevated,
        "a direct launch from here is what the shell detour avoids"
    );

    close(&wt, admin_window);
    close(&wt, user_window);
}
