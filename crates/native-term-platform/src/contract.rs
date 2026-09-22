//! What every backend must do, exercised against a live one: the tests
//! of each backend crate call `exercise` (marked `#[ignore]` where a real
//! terminal has to be running), and the fake runs it always.

use std::collections::HashSet;
use std::time::Duration;

use crate::{TabSpec, Target, TerminalBackend};

const WAIT: Duration = Duration::from_secs(30);

fn spec(label: &str) -> TabSpec {
    TabSpec {
        terminal_session: format!("00000000-0000-4000-8000-{:012x}", label.len()),
        label: label.to_string(),
        session: format!("contract-{label}"),
        alias: "nativeterm-test.invalid".into(),
        wait: true,
        no_forwards: false,
        tab_color: None,
    }
}

/// Open a new window with two tabs, find them, select the second, close
/// both, see the window go; open a tool tab and see its title. Panics
/// with what went wrong.
pub fn exercise(backend: &dyn TerminalBackend, labels: [&str; 2]) {
    let specs: Vec<TabSpec> = labels.iter().map(|l| spec(l)).collect();
    let wanted: HashSet<String> = labels.iter().map(|l| l.to_string()).collect();
    let expected: Vec<String> = labels.iter().map(|l| l.to_string()).collect();

    let report = backend.open(&Target::NewWindow, &specs).expect("open");
    assert_eq!(report.launched, 2, "both tabs handed to the terminal");
    assert!(report.pending.is_empty(), "nothing left over");
    let window = report.window.expect("a new window is reported");
    assert!(backend.window_ids().contains(&window), "the new window is listed");

    let (snapshot, missing) = backend.wait_for(&wanted, &expected, WAIT);
    assert!(missing.is_empty(), "tabs never claimed: {missing:?}");
    assert!(snapshot.complete);
    for label in labels {
        let (w, _) = snapshot.find(label).expect(label);
        assert_eq!(w.handle, window, "{label} is in the new window");
    }

    let (w, second) = snapshot.find(labels[1]).unwrap();
    let second = second.clone();
    assert!(backend.select(w.handle, &second).expect("select"), "the tab is still where it was");
    let snapshot = backend.snapshot(&wanted);
    let (_, tab) = snapshot.find(labels[1]).unwrap();
    assert!(tab.selected, "{} is selected after select", labels[1]);

    for label in labels {
        let snapshot = backend.snapshot(&wanted);
        let (w, tab) = snapshot.find(label).expect(label);
        let tab = tab.clone();
        assert!(backend.close(w.handle, &tab).expect("close"), "{label} closed");
    }
    let started = std::time::Instant::now();
    while backend.window_ids().contains(&window) {
        assert!(started.elapsed() < WAIT, "the window stays after its last tab closed");
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(backend.snapshot(&wanted).claimed().next().is_none(), "no claims are left");

    backend.open_tool("contract tool", &["--version".into()]).expect("open_tool");
    let started = std::time::Instant::now();
    loop {
        let snapshot = backend.snapshot(&HashSet::new());
        if snapshot.windows.iter().any(|w| w.tabs.iter().any(|t| t.name == "contract tool")) {
            break;
        }
        assert!(started.elapsed() < WAIT, "the tool tab never showed its title");
        std::thread::sleep(Duration::from_millis(100));
    }
}
