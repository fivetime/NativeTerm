//! Print Terminal change notifications for a while (diagnostics).
//!
//! `cargo run -p native-term-platform --example watch_events -- <terminal-dir> <seconds>`
#![cfg_attr(not(windows), allow(dead_code, unused_imports))]

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[cfg(windows)]
use native_term_platform::windows_terminal::events::{Change, Watcher};
#[cfg(windows)]
use native_term_platform::windows_terminal::install::Install;

#[cfg(not(windows))]
fn main() {}

#[cfg(windows)]
fn main() {
    let mut args = std::env::args().skip(1);
    let install = Install::from_dir(args.next().expect("terminal dir").as_ref()).unwrap();
    let seconds: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(20);
    let started = Instant::now();
    let watcher = Watcher::start(
        install,
        Arc::new(move |change: Change| {
            if !matches!(change, Change::Foreground | Change::Moved | Change::Popup) {
                println!("{:>7.2} {change:?}", started.elapsed().as_secs_f64());
            }
        }),
    );
    let mut last = (0, 0, 0, 0, 0);
    while started.elapsed() < Duration::from_secs(seconds) {
        std::thread::sleep(Duration::from_secs(1));
        let c = watcher.counts();
        let now = (
            c.windows.load(Ordering::Relaxed),
            c.foreground.load(Ordering::Relaxed),
            c.selected.load(Ordering::Relaxed),
            c.structure.load(Ordering::Relaxed),
            c.moved.load(Ordering::Relaxed),
        );
        if now != last {
            println!(
                "{:>7.2} totals: windows {} foreground {} selected {} structure {} moved {}",
                started.elapsed().as_secs_f64(),
                now.0,
                now.1,
                now.2,
                now.3,
                now.4
            );
            last = now;
        }
    }
    println!("abandoned UIA threads: {}", watcher.abandoned());
    let senders = watcher.counts().senders.lock().unwrap();
    let mut list: Vec<_> = senders.iter().collect();
    list.sort();
    for ((change, kind, class), n) in list {
        println!("structure change {change} from control type {kind} class {class:?}: {n}");
    }
}
