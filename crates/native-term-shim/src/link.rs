//! The shim's connection to NativeTerm, kept alive in the background.
//! ssh keeps working when NativeTerm isn't running; the link retries and,
//! on every (re)connect, sends `Hello` and replays the latest state.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use native_term_session::pipe;
use native_term_session::protocol::{AppMessage, ShimMessage};

const RETRY: Duration = Duration::from_secs(2);
const TICK: Duration = Duration::from_millis(50);

pub struct Link {
    outbox: Sender<ShimMessage>,
    inbox: Receiver<AppMessage>,
    /// Set once the first connection said hello; stays set (NativeTerm may
    /// answer and hang up within milliseconds).
    reached: Arc<AtomicBool>,
    stopped: Arc<AtomicBool>,
}

/// What a newly (re)connected NativeTerm needs to know.
/// Replayed in this order: the current attempt, its login, its outcome.
#[derive(Default)]
struct Replay {
    connecting: Option<ShimMessage>,
    authenticated: bool,
    /// `Exited` or `Waiting`.
    outcome: Option<ShimMessage>,
}

impl Link {
    pub fn start(pipe_name: String, hello: ShimMessage) -> Link {
        let (outbox, outgoing) = mpsc::channel::<ShimMessage>();
        let (incoming, inbox) = mpsc::channel::<AppMessage>();
        let reached = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&reached);
        let replay = Arc::new(Mutex::new(Replay::default()));
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&stopped);
        std::thread::spawn(move || loop {
            if stop.load(Ordering::SeqCst) {
                return;
            }
            let connected = pipe::connect(&pipe_name, RETRY);
            crate::debug::log(format!("pipe connect: {:?}", connected.as_ref().map(|_| ())));
            let Ok(conn) = connected else {
                // messages sent while away only update the replay state
                while let Ok(m) = outgoing.try_recv() {
                    remember(&replay, &m);
                }
                std::thread::sleep(RETRY);
                continue;
            };
            let mut ok = conn.send(&hello).is_ok();
            if ok {
                let state = replay.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(m) = &state.connecting {
                    ok &= conn.send(m).is_ok();
                }
                if state.authenticated {
                    ok &= conn.send(&ShimMessage::Authenticated).is_ok();
                }
                if let Some(m) = &state.outcome {
                    ok &= conn.send(m).is_ok();
                }
            }
            if ok {
                flag.store(true, Ordering::SeqCst);
            }
            while ok && !stop.load(Ordering::SeqCst) {
                while let Ok(m) = outgoing.try_recv() {
                    remember(&replay, &m);
                    ok &= conn.send(&m).is_ok();
                }
                match conn.recv::<AppMessage>(TICK) {
                    Ok(Some(m)) => {
                        if incoming.send(m).is_err() {
                            return;
                        }
                    }
                    Ok(None) => {}
                    Err(_) => ok = false,
                }
            }
            // a failed write doesn't mean nothing is left to read: NativeTerm
            // may have sent a command right before hanging up
            while let Ok(Some(m)) = conn.recv::<AppMessage>(Duration::ZERO) {
                if incoming.send(m).is_err() {
                    return;
                }
            }
            crate::debug::log("pipe connection ended");
            drop(conn);
            // NativeTerm restarting, or hanging up on us: don't spin
            std::thread::sleep(RETRY);
        });
        Link { outbox, inbox, reached, stopped }
    }

    pub fn send(&self, message: ShimMessage) {
        let _ = self.outbox.send(message);
    }

    pub fn sender(&self) -> Sender<ShimMessage> {
        self.outbox.clone()
    }

    pub fn recv(&self, timeout: Duration) -> Option<AppMessage> {
        self.inbox.recv_timeout(timeout).ok()
    }

    /// Whether NativeTerm has been reached (now or before), waiting up to
    /// `timeout`. Its answers are in the inbox even if it hung up since.
    pub fn wait_reached(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.reached.load(Ordering::SeqCst) {
                return true;
            }
            std::thread::sleep(TICK);
        }
        self.reached.load(Ordering::SeqCst)
    }
}

impl Drop for Link {
    /// Disconnects within a tick; the tab is no longer NativeTerm's.
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
    }
}

fn remember(replay: &Mutex<Replay>, message: &ShimMessage) {
    let mut state = replay.lock().unwrap_or_else(|e| e.into_inner());
    match message {
        ShimMessage::Connecting { .. } => {
            *state = Replay { connecting: Some(message.clone()), ..Replay::default() };
        }
        ShimMessage::Waiting => {
            *state = Replay { outcome: Some(message.clone()), ..Replay::default() };
        }
        ShimMessage::Exited { .. } => state.outcome = Some(message.clone()),
        ShimMessage::Authenticated => state.authenticated = true,
        _ => {}
    }
}
