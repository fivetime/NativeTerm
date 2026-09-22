//! A terminal that exists only in memory, for testing the program without
//! one. Windows and tabs are plain data: `open` makes tabs titled with
//! their label (what the claimer's first rule looks for), `select` and
//! `close` change them, and a test drives the rest by hand — which window
//! is in front, tabs the user opened or renamed, tabs that never appear,
//! change notifications. Every call the program makes is recorded.
//!
//! Claims go through the real `Claimer`, so what the program sees is what
//! a terminal without tab rectangles would give it.

use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use crate::backend::{Capabilities, Change, ChangeCounts, Notify, OpenReport, Subscription, TerminalBackend};
use crate::claim::{Claimer, WindowTabs};
use crate::{Snapshot, TabSpec, TabView, Target, WindowId, WindowView};

/// One tab of the fake terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FakeTab {
    pub title: String,
    /// What `open` was asked for, if the program opened it.
    pub spec: Option<TabSpec>,
}

/// One window of the fake terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FakeWindow {
    pub id: WindowId,
    pub tabs: Vec<FakeTab>,
    pub selected: usize,
}

/// What the program asked of the terminal, in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Calls {
    pub opened: Vec<(Target, Vec<TabSpec>)>,
    pub tools: Vec<(String, Vec<String>)>,
    pub selected: Vec<(WindowId, usize)>,
    pub closed: Vec<(WindowId, usize)>,
    pub activated: Vec<WindowId>,
}

#[derive(Default)]
struct State {
    windows: Vec<FakeWindow>,
    next_window: u64,
    foreground: Option<WindowId>,
    /// What `Target::Recent` means: the last window activated or made.
    recent: Option<WindowId>,
    named: HashMap<String, WindowId>,
    /// How many of the next `open`'s tabs are launched but never appear.
    drop_next: usize,
    calls: Calls,
}

struct Subscriber {
    notify: Notify,
    alive: Arc<AtomicBool>,
    counts: Arc<Mutex<ChangeCounts>>,
}

struct Inner {
    shim: PathBuf,
    state: Mutex<State>,
    claimer: Mutex<Claimer>,
    subscribers: Mutex<Vec<Subscriber>>,
}

/// The fake terminal. Cloning gives another handle to the same one: the
/// test keeps a clone and hands the other to the program.
#[derive(Clone)]
pub struct FakeBackend(Arc<Inner>);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl FakeBackend {
    /// A terminal with no windows whose tabs would run `shim`.
    pub fn new(shim: impl Into<PathBuf>) -> FakeBackend {
        FakeBackend(Arc::new(Inner {
            shim: shim.into(),
            state: Mutex::new(State { next_window: 1, ..State::default() }),
            claimer: Mutex::new(Claimer::new()),
            subscribers: Mutex::new(Vec::new()),
        }))
    }

    /// The windows as they are now.
    pub fn windows(&self) -> Vec<FakeWindow> {
        lock(&self.0.state).windows.clone()
    }

    pub fn calls(&self) -> Calls {
        lock(&self.0.state).calls.clone()
    }

    /// The next `open` launches its tabs, but the first `count` of them
    /// never show up (the terminal lost them).
    pub fn drop_next(&self, count: usize) {
        lock(&self.0.state).drop_next = count;
    }

    /// The user brought a window forward (or something else, `None`).
    pub fn set_foreground(&self, window: Option<WindowId>) {
        let mut state = lock(&self.0.state);
        state.foreground = window;
        if window.is_some() {
            state.recent = window;
        }
    }

    /// The user opened a tab of their own in `window`; it is selected.
    pub fn add_tab(&self, window: WindowId, title: &str) {
        let mut state = lock(&self.0.state);
        if let Some(w) = state.windows.iter_mut().find(|w| w.id == window) {
            w.tabs.push(FakeTab { title: title.to_string(), spec: None });
            w.selected = w.tabs.len() - 1;
        }
    }

    /// A tab's title changed (the program running in it set one).
    pub fn rename_tab(&self, window: WindowId, index: usize, title: &str) {
        let mut state = lock(&self.0.state);
        if let Some(tab) = state.windows.iter_mut().find(|w| w.id == window).and_then(|w| w.tabs.get_mut(index)) {
            tab.title = title.to_string();
        }
    }

    /// The user selected a tab.
    pub fn select_tab(&self, window: WindowId, index: usize) {
        let mut state = lock(&self.0.state);
        if let Some(w) = state.windows.iter_mut().find(|w| w.id == window && index < w.tabs.len()) {
            w.selected = index;
        }
    }

    /// The user closed a window with everything in it.
    pub fn close_window(&self, window: WindowId) {
        let mut state = lock(&self.0.state);
        state.windows.retain(|w| w.id != window);
        if state.foreground == Some(window) {
            state.foreground = None;
        }
        if state.recent == Some(window) {
            state.recent = state.windows.last().map(|w| w.id);
        }
    }

    /// Tell every subscriber something changed.
    pub fn emit(&self, change: Change) {
        let subscribers = lock(&self.0.subscribers);
        for s in subscribers.iter().filter(|s| s.alive.load(Ordering::Relaxed)) {
            {
                let mut counts = lock(&s.counts);
                match change {
                    Change::Windows => counts.windows += 1,
                    Change::Tabs | Change::Content => counts.tabs += 1,
                    Change::Foreground | Change::Popup | Change::Moved => {}
                }
            }
            (s.notify)(change);
        }
    }

    /// Subscriptions that haven't been dropped.
    pub fn subscribers(&self) -> usize {
        lock(&self.0.subscribers).iter().filter(|s| s.alive.load(Ordering::Relaxed)).count()
    }

    fn make_window(state: &mut State) -> WindowId {
        let id = WindowId(state.next_window);
        state.next_window += 1;
        state.windows.push(FakeWindow { id, tabs: Vec::new(), selected: 0 });
        state.recent = Some(id);
        id
    }

    /// The window `Target::Recent` means, made if there is none.
    fn recent_window(state: &mut State) -> WindowId {
        match state.recent.filter(|id| state.windows.iter().any(|w| w.id == *id)) {
            Some(id) => id,
            None => match state.windows.last() {
                Some(w) => w.id,
                None => Self::make_window(state),
            },
        }
    }

    /// Whether `tab` is still where the snapshot saw it.
    fn matching(state: &State, window: WindowId, tab: &TabView) -> io::Result<bool> {
        let w = state
            .windows
            .iter()
            .find(|w| w.id == window)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such window"))?;
        Ok(w.tabs.get(tab.index).is_some_and(|t| t.title == tab.name))
    }
}

struct FakeSubscription {
    alive: Arc<AtomicBool>,
    counts: Arc<Mutex<ChangeCounts>>,
}

impl Subscription for FakeSubscription {
    fn counts(&self) -> ChangeCounts {
        *lock(&self.counts)
    }
}

impl Drop for FakeSubscription {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Relaxed);
    }
}

impl TerminalBackend for FakeBackend {
    fn name(&self) -> &'static str {
        "a terminal in memory"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities { named_windows: true, ..Capabilities::default() }
    }

    fn shim_path(&self) -> &Path {
        &self.0.shim
    }

    fn window_ids(&self) -> Vec<WindowId> {
        lock(&self.0.state).windows.iter().map(|w| w.id).collect()
    }

    fn foreground(&self) -> Option<WindowId> {
        lock(&self.0.state).foreground
    }

    fn activate(&self, window: WindowId) -> bool {
        let mut state = lock(&self.0.state);
        state.calls.activated.push(window);
        if !state.windows.iter().any(|w| w.id == window) {
            return false;
        }
        state.foreground = Some(window);
        state.recent = Some(window);
        true
    }

    fn snapshot(&self, labels: &HashSet<String>) -> Snapshot {
        let state = lock(&self.0.state);
        let mut claimer = lock(&self.0.claimer);
        let windows = state
            .windows
            .iter()
            .map(|w| {
                let tabs = WindowTabs {
                    names: w.tabs.iter().map(|t| t.title.clone()).collect(),
                    rects: vec![None; w.tabs.len()],
                    selected: (!w.tabs.is_empty()).then_some(w.selected),
                    panes: Vec::new(),
                };
                WindowView {
                    handle: w.id,
                    pid: 1,
                    foreground: state.foreground == Some(w.id),
                    unresponsive: false,
                    tabs: claimer.claim(w.id, &tabs, labels),
                }
            })
            .collect();
        claimer.retain(&state.windows.iter().map(|w| w.id).collect::<Vec<_>>());
        Snapshot { windows, complete: true }
    }

    fn open(&self, target: &Target, tabs: &[TabSpec]) -> io::Result<OpenReport> {
        let mut state = lock(&self.0.state);
        state.calls.opened.push((target.clone(), tabs.to_vec()));
        let (window, made) = match target {
            Target::NewWindow => (Self::make_window(&mut state), true),
            Target::Recent => (Self::recent_window(&mut state), false),
            Target::Named(name) => {
                match state.named.get(name).copied().filter(|id| state.windows.iter().any(|w| w.id == *id)) {
                    Some(id) => (id, false),
                    None => {
                        let id = Self::make_window(&mut state);
                        state.named.insert(name.clone(), id);
                        (id, true)
                    }
                }
            }
        };
        let dropped = std::mem::take(&mut state.drop_next);
        let w = state.windows.iter_mut().find(|w| w.id == window).expect("just found or made");
        for spec in tabs.iter().skip(dropped) {
            w.tabs.push(FakeTab { title: spec.label.clone(), spec: Some(spec.clone()) });
            w.selected = w.tabs.len() - 1;
        }
        state.recent = Some(window);
        Ok(OpenReport { launched: tabs.len(), pending: Vec::new(), window: made.then_some(window) })
    }

    fn open_tool(&self, title: &str, shim_args: &[String]) -> io::Result<()> {
        let mut state = lock(&self.0.state);
        state.calls.tools.push((title.to_string(), shim_args.to_vec()));
        let window = Self::recent_window(&mut state);
        let w = state.windows.iter_mut().find(|w| w.id == window).expect("just found or made");
        w.tabs.push(FakeTab { title: title.to_string(), spec: None });
        w.selected = w.tabs.len() - 1;
        Ok(())
    }

    fn select(&self, window: WindowId, tab: &TabView) -> io::Result<bool> {
        let mut state = lock(&self.0.state);
        state.calls.selected.push((window, tab.index));
        if !Self::matching(&state, window, tab)? {
            return Ok(false);
        }
        let w = state.windows.iter_mut().find(|w| w.id == window).expect("matched");
        w.selected = tab.index;
        state.foreground = Some(window);
        state.recent = Some(window);
        Ok(true)
    }

    fn close(&self, window: WindowId, tab: &TabView) -> io::Result<bool> {
        let mut state = lock(&self.0.state);
        state.calls.closed.push((window, tab.index));
        if !Self::matching(&state, window, tab)? {
            return Ok(false);
        }
        let w = state.windows.iter_mut().find(|w| w.id == window).expect("matched");
        w.tabs.remove(tab.index);
        w.selected = w.selected.min(w.tabs.len().saturating_sub(1));
        if w.tabs.is_empty() {
            state.windows.retain(|w| w.id != window);
            if state.foreground == Some(window) {
                state.foreground = None;
            }
            if state.recent == Some(window) {
                state.recent = state.windows.last().map(|w| w.id);
            }
        }
        Ok(true)
    }

    fn subscribe(&self, notify: Notify) -> Box<dyn Subscription> {
        let alive = Arc::new(AtomicBool::new(true));
        let counts = Arc::new(Mutex::new(ChangeCounts::default()));
        lock(&self.0.subscribers).push(Subscriber { notify, alive: Arc::clone(&alive), counts: Arc::clone(&counts) });
        Box::new(FakeSubscription { alive, counts })
    }

    /// Nothing arrives later than `snapshot` shows: no waiting.
    fn wait_for(&self, labels: &HashSet<String>, expected: &[String], _timeout: Duration) -> (Snapshot, Vec<String>) {
        let snapshot = self.snapshot(labels);
        let missing = expected.iter().filter(|l| snapshot.find(l).is_none()).cloned().collect();
        (snapshot, missing)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(label: &str) -> TabSpec {
        TabSpec {
            terminal_session: format!("guid-{label}"),
            label: label.to_string(),
            session: format!("id-{label}"),
            alias: "host".into(),
            wait: false,
            no_forwards: false,
            tab_color: None,
        }
    }

    fn labels(l: &[&str]) -> HashSet<String> {
        l.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn meets_the_contract() {
        crate::contract::exercise(&FakeBackend::new("shim"), ["one", "two"]);
    }

    #[test]
    fn recent_is_the_last_window_activated_or_made() {
        let fake = FakeBackend::new("shim");
        let a = fake.open(&Target::NewWindow, &[spec("a")]).unwrap().window.unwrap();
        let b = fake.open(&Target::NewWindow, &[spec("b")]).unwrap().window.unwrap();
        assert_ne!(a, b);
        fake.open(&Target::Recent, &[spec("c")]).unwrap();
        assert_eq!(fake.windows()[1].tabs.len(), 2, "into the newest window");
        assert!(fake.activate(a));
        fake.open(&Target::Recent, &[spec("d")]).unwrap();
        assert_eq!(fake.windows()[0].tabs.len(), 2, "into the activated one");
        assert_eq!(fake.foreground(), Some(a));
        assert_eq!(fake.calls().activated, [a]);
    }

    #[test]
    fn named_windows_are_found_again() {
        let fake = FakeBackend::new("shim");
        let first = fake.open(&Target::Named("w".into()), &[spec("a")]).unwrap();
        let again = fake.open(&Target::Named("w".into()), &[spec("b")]).unwrap();
        assert!(first.window.is_some());
        assert_eq!(again.window, None, "an existing window isn't reported as new");
        assert_eq!(fake.windows().len(), 1);
        assert_eq!(fake.windows()[0].tabs.len(), 2);
    }

    #[test]
    fn dropped_tabs_are_launched_but_never_appear() {
        let fake = FakeBackend::new("shim");
        fake.drop_next(1);
        let report = fake.open(&Target::NewWindow, &[spec("a"), spec("b")]).unwrap();
        assert_eq!(report.launched, 2);
        let (snapshot, missing) = fake.wait_for(&labels(&["a", "b"]), &["a".into(), "b".into()], Duration::ZERO);
        assert_eq!(missing, ["a"]);
        assert!(snapshot.find("b").is_some());
        // only once
        fake.open(&Target::Recent, &[spec("a")]).unwrap();
        assert_eq!(fake.windows()[0].tabs.len(), 2);
    }

    #[test]
    fn select_and_close_check_the_tab_is_still_there() {
        let fake = FakeBackend::new("shim");
        let w = fake.open(&Target::NewWindow, &[spec("a"), spec("b")]).unwrap().window.unwrap();
        let snapshot = fake.snapshot(&labels(&["a", "b"]));
        let (_, a) = snapshot.find("a").unwrap();
        let a = a.clone();
        fake.rename_tab(w, 0, "a: vim");
        assert!(!fake.select(w, &a).unwrap(), "renamed since");
        assert!(!fake.close(w, &a).unwrap());
        fake.rename_tab(w, 0, "a");
        assert!(fake.select(w, &a).unwrap());
        assert_eq!(fake.windows()[0].selected, 0);
        assert!(fake.close(w, &a).unwrap());
        assert_eq!(fake.windows()[0].tabs.len(), 1);
        assert!(matches!(fake.select(WindowId(99), &a), Err(e) if e.kind() == io::ErrorKind::NotFound));
    }

    #[test]
    fn subscribers_hear_changes_until_dropped() {
        let fake = FakeBackend::new("shim");
        let heard = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&heard);
        let sub = fake.subscribe(Arc::new(move |c| lock(&sink).push(c)));
        fake.emit(Change::Windows);
        fake.emit(Change::Tabs);
        fake.emit(Change::Foreground);
        assert_eq!(*lock(&heard), [Change::Windows, Change::Tabs, Change::Foreground]);
        assert_eq!(sub.counts(), ChangeCounts { windows: 1, tabs: 1 });
        assert_eq!(fake.subscribers(), 1);
        drop(sub);
        fake.emit(Change::Windows);
        assert_eq!(lock(&heard).len(), 3);
        assert_eq!(fake.subscribers(), 0);
    }

    #[test]
    fn the_users_own_tabs_and_windows() {
        let fake = FakeBackend::new("shim");
        let w = fake.open(&Target::NewWindow, &[spec("a")]).unwrap().window.unwrap();
        fake.add_tab(w, "pwsh");
        fake.select_tab(w, 0);
        let snapshot = fake.snapshot(&labels(&["a"]));
        let tabs = &snapshot.windows[0].tabs;
        assert_eq!(tabs.len(), 2);
        assert!(tabs[0].selected && tabs[0].claim.is_some());
        assert!(tabs[1].claim.is_none() && tabs[1].rect.is_none());
        fake.set_foreground(Some(w));
        assert!(fake.snapshot(&labels(&["a"])).windows[0].foreground);
        fake.close_window(w);
        assert!(fake.window_ids().is_empty());
        assert_eq!(fake.foreground(), None);
    }
}
