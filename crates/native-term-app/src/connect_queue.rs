//! Logins one after another: opening a folder, "Connect all", and
//! automatic reconnects (many sessions drop together after sleep) all go
//! through this queue, so a jump host doesn't get dozens of logins at once
//! (sshd's `MaxStartups`). A connection starts when its shim is there,
//! at most [`MAX_CONNECTING`] are logging in at a time, and starts are
//! spaced by [`SPACING`].

use std::sync::mpsc::Receiver;
use std::sync::Weak;
use std::time::{Duration, Instant};

use crate::{lock, AppMessage, Shared, State};

pub(crate) const SPACING: Duration = Duration::from_millis(200);
pub(crate) const MAX_CONNECTING: usize = 4;
/// A shim that isn't there by then won't be connected by the queue.
const SHIM_WAIT: Duration = Duration::from_secs(30);
/// A login that takes longer doesn't hold the others back.
const SLOT_WAIT: Duration = Duration::from_secs(20);
const POLL: Duration = Duration::from_millis(100);

enum Ready {
    Yes,
    NotYet,
    /// Closed, gone, or already connecting: nothing to do.
    Never,
}

fn ready(shared: &Shared, id: &str) -> Ready {
    let sessions = lock(&shared.sessions);
    match sessions.iter().find(|s| s.id == id) {
        None => Ready::Never,
        Some(s) if !s.state.is_open() => Ready::Never,
        Some(s) if s.link.is_some() && s.state.can_connect() => Ready::Yes,
        Some(s) if matches!(s.state, State::Connecting | State::Connected) => Ready::Never,
        Some(_) => Ready::NotYet,
    }
}

fn connecting(shared: &Shared) -> usize {
    lock(&shared.sessions).iter().filter(|s| s.state == State::Connecting).count()
}

pub(crate) fn run(shared: Weak<Shared>, ids: Receiver<String>) {
    let mut pending: Vec<(String, Instant)> = Vec::new();
    loop {
        // take what arrived; block only when there's nothing to do
        if pending.is_empty() {
            match ids.recv() {
                Ok(id) => pending.push((id, Instant::now())),
                Err(_) => return,
            }
        }
        while let Ok(id) = ids.try_recv() {
            if !pending.iter().any(|(p, _)| *p == id) {
                pending.push((id, Instant::now()));
            }
        }
        let Some(shared) = shared.upgrade() else { return };
        // the first ready session, in order; drop the hopeless ones
        let mut chosen = None;
        pending.retain(|(id, since)| {
            if chosen.is_some() {
                return true;
            }
            match ready(&shared, id) {
                Ready::Yes => {
                    chosen = Some((id.clone(), *since));
                    false
                }
                Ready::NotYet => since.elapsed() < SHIM_WAIT,
                Ready::Never => false,
            }
        });
        let Some((id, since)) = chosen else {
            drop(shared);
            std::thread::sleep(POLL);
            continue;
        };
        let started = Instant::now();
        while connecting(&shared) >= MAX_CONNECTING && started.elapsed() < SLOT_WAIT {
            std::thread::sleep(POLL);
        }
        let link = lock(&shared.sessions).iter().find(|s| s.id == id).and_then(|s| s.link.clone());
        let told = link.is_some_and(|link| link.send(&AppMessage::Connect).is_ok());
        if told {
            // counted as connecting right away, not only when the shim says so
            if let Some(s) = lock(&shared.sessions).iter_mut().find(|s| s.id == id) {
                s.told_to_connect = true;
                if s.state.can_connect() {
                    s.state = State::Connecting;
                }
            }
            shared.changed();
        } else if since.elapsed() < SHIM_WAIT {
            // the link went away between looking and telling (a shim whose
            // connection broke comes back): it keeps its place in the queue
            // instead of waiting for a word that will never come
            pending.insert(0, (id, since));
        }
        drop(shared);
        std::thread::sleep(SPACING);
    }
}
