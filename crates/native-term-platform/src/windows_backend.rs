//! Windows Terminal backend: tabs are opened with `wt -w 0 new-tab ...`
//! (the most recently used window; optionally a dedicated window named
//! `DEDICATED_WINDOW_NAME`), and listed/selected/closed through UI
//! Automation across all Windows Terminal windows.
//!
//! Not yet implemented — see `docs/ROADMAP.md` Phase 0.

use crate::{TabInfo, TerminalBackend};

pub const DEDICATED_WINDOW_NAME: &str = "NativeTerm";

pub struct WindowsTerminalBackend;

impl TerminalBackend for WindowsTerminalBackend {
    fn open_tab(&self, _session_id: &str, _title: &str, _host_alias: &str) -> std::io::Result<()> {
        todo!(
            "wt -w 0 new-tab --sessionId {{<guid>}} --title <title> --suppressApplicationTitle \
             --profile \"NativeTerm SSH\" nativeterm-shim <host-alias>"
        )
    }

    fn list_tabs(&self) -> std::io::Result<Vec<TabInfo>> {
        todo!("enumerate tab strips of all Windows Terminal windows via UI Automation")
    }

    fn focus_tab(&self, _title: &str) -> std::io::Result<()> {
        todo!("find the tab via UIA and select it")
    }

    fn close_tab(&self, _title: &str) -> std::io::Result<()> {
        todo!("fallback only: normally the shim exits 0 and Windows Terminal closes the tab")
    }
}
