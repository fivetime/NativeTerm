//! Running `wt`. An elevated `wt` joins the elevated Terminal instance, not
//! the user's window (see "Elevation decides which Terminal instance a tab
//! joins"), so an elevated NativeTerm asks the desktop shell to start it:
//! the shell runs with the user's normal token.

use std::ffi::OsString;
use std::io;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use windows::core::{Interface, BSTR};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, IDispatch, IServiceProvider, CLSCTX_LOCAL_SERVER,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Shell::{
    IShellBrowser, IShellDispatch2, IShellFolderViewDual, IShellView, IShellWindows, ShellWindows,
    SID_STopLevelBrowser, SVGIO_BACKGROUND, SWC_DESKTOP, SWFO_NEEDDISPATCH,
};
use windows::Win32::UI::WindowsAndMessaging::{AllowSetForegroundWindow, ASFW_ANY, SW_SHOWNORMAL};

use super::command;

const WAIT: Duration = Duration::from_secs(10);

/// Set to `1` to always launch through the shell (for tests).
pub const VIA_SHELL_ENV: &str = "NATIVETERM_LAUNCH_VIA_SHELL";

pub fn run(launcher: &Path, args: &[OsString]) -> io::Result<()> {
    if native_term_win::is_elevated() || std::env::var(VIA_SHELL_ENV).as_deref() == Ok("1") {
        let (launcher, line) = (launcher.to_path_buf(), command::join(args));
        // the shell isn't the foreground app: let the new window come forward
        unsafe {
            let _ = AllowSetForegroundWindow(ASFW_ANY);
        }
        // the shell objects want a single-threaded apartment of their own
        return std::thread::spawn(move || shell_execute(&launcher, &line))
            .join()
            .unwrap_or_else(|_| Err(io::Error::other("shell launch panicked")));
    }
    let mut child = Command::new(launcher).args(args).spawn()?;
    // wt.exe hands the command to the running instance and exits
    let started = Instant::now();
    while started.elapsed() < WAIT {
        if child.try_wait()?.is_some() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

/// `ShellExecute` through the desktop's shell view (Explorer's process).
fn shell_execute(file: &Path, parameters: &str) -> io::Result<()> {
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        let result = (|| -> windows::core::Result<()> {
            let windows: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_LOCAL_SERVER)?;
            let location = VARIANT::from(0i32); // CSIDL_DESKTOP
            let empty = VARIANT::default();
            let mut handle = 0i32;
            let desktop: IDispatch =
                windows.FindWindowSW(&location, &empty, SWC_DESKTOP, &mut handle, SWFO_NEEDDISPATCH)?;
            let provider: IServiceProvider = desktop.cast()?;
            let browser: IShellBrowser = provider.QueryService(&SID_STopLevelBrowser)?;
            let view: IShellView = browser.QueryActiveShellView()?;
            let background: IDispatch = view.GetItemObject(SVGIO_BACKGROUND)?;
            let folder_view: IShellFolderViewDual = background.cast()?;
            let shell: IShellDispatch2 = folder_view.Application()?.cast()?;
            let dir = file.parent().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
            shell.ShellExecute(
                &BSTR::from(file.to_string_lossy().as_ref()),
                &VARIANT::from(BSTR::from(parameters)),
                &VARIANT::from(BSTR::from(dir)),
                &VARIANT::from(BSTR::from("open")),
                &VARIANT::from(SW_SHOWNORMAL.0),
            )
        })();
        CoUninitialize();
        result.map_err(io::Error::from)
    }
}
