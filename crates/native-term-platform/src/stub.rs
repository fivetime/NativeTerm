//! No terminal at all: what the program runs against on a system whose
//! terminal it can't drive yet. Every operation says so
//! (`io::ErrorKind::Unsupported`), there are no windows, and nothing is
//! ever claimed, so the program shows its list and settings and can't
//! open a tab.

use std::any::Any;
use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};

use crate::backend::{Capabilities, ChangeCounts, Notify, OpenReport, Subscription, TerminalBackend};
use crate::{Snapshot, TabSpec, TabView, Target, WindowId};

pub struct NoTerminal {
    shim: PathBuf,
}

impl NoTerminal {
    pub fn new(shim: impl Into<PathBuf>) -> NoTerminal {
        NoTerminal { shim: shim.into() }
    }
}

fn unsupported() -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, "no terminal can be driven on this system yet")
}

struct Silence;

impl Subscription for Silence {
    fn counts(&self) -> ChangeCounts {
        ChangeCounts::default()
    }
}

impl TerminalBackend for NoTerminal {
    fn name(&self) -> &'static str {
        "no terminal"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }

    fn shim_path(&self) -> &Path {
        &self.shim
    }

    fn window_ids(&self) -> Vec<WindowId> {
        Vec::new()
    }

    fn foreground(&self) -> Option<WindowId> {
        None
    }

    fn activate(&self, _window: WindowId) -> bool {
        false
    }

    fn snapshot(&self, _labels: &HashSet<String>) -> Snapshot {
        Snapshot { windows: Vec::new(), complete: true }
    }

    fn open(&self, _target: &Target, _tabs: &[TabSpec]) -> io::Result<OpenReport> {
        Err(unsupported())
    }

    fn open_tool(&self, _title: &str, _shim_args: &[String]) -> io::Result<()> {
        Err(unsupported())
    }

    fn select(&self, _window: WindowId, _tab: &TabView) -> io::Result<bool> {
        Err(unsupported())
    }

    fn close(&self, _window: WindowId, _tab: &TabView) -> io::Result<bool> {
        Err(unsupported())
    }

    fn subscribe(&self, _notify: Notify) -> Box<dyn Subscription> {
        Box::new(Silence)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_no_to_everything() {
        let none = NoTerminal::new("shim");
        assert_eq!(none.capabilities(), Capabilities::default());
        assert!(none.window_ids().is_empty());
        assert_eq!(none.open(&Target::NewWindow, &[]).unwrap_err().kind(), io::ErrorKind::Unsupported);
        assert!(none.snapshot(&HashSet::new()).complete);
        assert_eq!(none.subscribe(std::sync::Arc::new(|_| {})).counts(), ChangeCounts::default());
    }
}
