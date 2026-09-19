//! Snapshots of Terminal tabs for the tab list: the selected tab of each
//! Terminal window, pictured when a scan finds it (scans follow Terminal's
//! change notifications: a tab or window switched, opened, closed), never
//! on a timer. Terminal renders only the selected tab, so a tab's picture
//! is how it looked when it was last seen selected. Kept in memory only.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use native_term_platform::windows_terminal::capture::{self, Image};
use native_term_platform::Snapshot;

/// A tab seen this recently isn't pictured again.
const FRESH: Duration = Duration::from_secs(3);

/// Pictures this wide (physical pixels).
const WIDTH: i32 = 360;

#[derive(Clone, Debug)]
pub struct Preview {
    /// The tab's title then.
    pub title: String,
    pub taken: SystemTime,
    at: Instant,
    pub image: Arc<Image>,
}

/// By window and tab position.
#[derive(Default)]
pub struct Previews {
    map: HashMap<(isize, usize), Preview>,
}

impl Previews {
    pub fn get(&self, window: isize, index: usize) -> Option<Preview> {
        self.map.get(&(window, index)).cloned()
    }

    /// Forget the pictures that no longer fit their place: the window or
    /// the tab is gone, or the tabs moved (their count changed and the
    /// title there isn't the pictured one). An incomplete scan forgets
    /// nothing of the windows it didn't read.
    pub fn prune(&mut self, before: &Snapshot, now: &Snapshot) {
        self.map.retain(|(window, index), p| {
            let Some(w) = now.windows.iter().find(|w| w.handle == *window) else { return !now.complete };
            if w.unresponsive {
                return true;
            }
            let Some(tab) = w.tabs.iter().find(|t| t.index == *index) else { return false };
            let count_before = before.windows.iter().find(|b| b.handle == *window).map(|b| b.tabs.len());
            tab.name == p.title || count_before == Some(w.tabs.len())
        });
    }

    /// The selected tabs that need a picture: (window, tab position,
    /// title, where the tab strip ends).
    pub fn wanted(&self, snapshot: &Snapshot) -> Vec<(isize, usize, String, Option<i32>)> {
        snapshot
            .windows
            .iter()
            .filter(|w| !w.unresponsive)
            .filter_map(|w| {
                let tab = w.tabs.iter().find(|t| t.selected)?;
                let fresh =
                    self.map.get(&(w.handle, tab.index)).is_some_and(|p| p.title == tab.name && p.at.elapsed() < FRESH);
                let strip = w.tabs.iter().filter_map(|t| t.rect).map(|r| r.bottom).max();
                (!fresh).then(|| (w.handle, tab.index, tab.name.clone(), strip))
            })
            .collect()
    }

    pub fn insert(&mut self, window: isize, index: usize, title: String, image: Image) {
        let preview = Preview { title, taken: SystemTime::now(), at: Instant::now(), image: Arc::new(image) };
        self.map.insert((window, index), preview);
    }
}

/// Picture what `wanted` asks for (on the scanning thread; ~25 ms a
/// window). Minimized windows are skipped. Whether any was taken.
pub fn take(previews: &std::sync::Mutex<Previews>, snapshot: &Snapshot) -> bool {
    let wanted = crate::lock(previews).wanted(snapshot);
    let mut taken = false;
    for (window, index, title, strip) in wanted {
        if let Some(image) = capture::capture(window, strip, WIDTH) {
            crate::lock(previews).insert(window, index, title, image);
            taken = true;
        }
    }
    taken
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_term_platform::{Rect, TabView, WindowView};

    fn tab(index: usize, name: &str, selected: bool) -> TabView {
        let rect = Rect { left: 0, top: 0, right: 100, bottom: 40 };
        TabView { index, name: name.into(), selected, rect: Some(rect), claim: None }
    }

    fn window(handle: isize, tabs: Vec<TabView>) -> WindowView {
        WindowView { handle, pid: 1, foreground: false, unresponsive: false, tabs }
    }

    fn snapshot(windows: Vec<WindowView>) -> Snapshot {
        Snapshot { windows, complete: true }
    }

    fn image() -> Image {
        Image { width: 1, height: 1, rgba: vec![0, 0, 0, 255] }
    }

    #[test]
    fn the_selected_tabs_are_pictured_once() {
        let now = snapshot(vec![
            window(1, vec![tab(0, "pwsh", false), tab(1, "web01", true)]),
            window(2, vec![tab(0, "claude", true)]),
        ]);
        let mut previews = Previews::default();
        let wanted = previews.wanted(&now);
        assert_eq!(wanted, vec![(1, 1, "web01".into(), Some(40)), (2, 0, "claude".into(), Some(40))]);
        previews.insert(1, 1, "web01".into(), image());
        assert_eq!(previews.wanted(&now).len(), 1, "web01 was just pictured");
        // a new title there: pictured again
        let renamed = snapshot(vec![window(1, vec![tab(0, "pwsh", false), tab(1, "web01: top", true)])]);
        assert_eq!(previews.wanted(&renamed).len(), 1);
    }

    #[test]
    fn pictures_that_no_longer_fit_are_forgotten() {
        let before = snapshot(vec![
            window(1, vec![tab(0, "pwsh", false), tab(1, "web01", false), tab(2, "db", true)]),
            window(2, vec![tab(0, "claude", true)]),
        ]);
        let mut previews = Previews::default();
        for (w, i, t) in [(1, 0, "pwsh"), (1, 1, "web01"), (1, 2, "db"), (2, 0, "claude")] {
            previews.insert(w, i, t.into(), image());
        }
        // the title changed, same tabs: kept
        let retitled = snapshot(vec![
            window(1, vec![tab(0, "pwsh: ls", false), tab(1, "web01", false), tab(2, "db", true)]),
            window(2, vec![tab(0, "claude", true)]),
        ]);
        previews.prune(&before, &retitled);
        assert!(previews.get(1, 0).is_some());
        // "pwsh" closed: the others moved up; window 2 closed
        let closed = snapshot(vec![window(1, vec![tab(0, "web01", false), tab(1, "db", true)])]);
        previews.prune(&retitled, &closed);
        assert!(previews.get(1, 0).is_none(), "web01's place had pwsh's picture");
        assert!(previews.get(1, 1).is_none());
        assert!(previews.get(1, 2).is_none(), "no tab there");
        assert!(previews.get(2, 0).is_none(), "window gone");
    }
}
