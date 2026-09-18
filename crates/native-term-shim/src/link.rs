//! The shim's connection to NativeTerm, kept alive in the background.
//! ssh keeps working when NativeTerm isn't running; the link retries and,
//! on every (re)connect, sends `Hello` and replays the latest state.
//!
//! Nothing polls: a reader thread blocks on the pipe, messages to NativeTerm
//! are written directly by the sending thread, and arrivals set an event the
//! shim's main loop waits on together with its other handles.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use native_term_session::pipe::{self, PipeConnection};
use native_term_session::protocol::{AppMessage, ShimMessage};

use crate::win::Event;

const RETRY: Duration = Duration::from_secs(2);
const TICK: Duration = Duration::from_millis(50);
/// A blocked read wakes this rarely even when nothing arrives.
const READ_WAIT: Duration = Duration::from_secs(3600);

/// Replayed in this order: the current attempt, its login, its silence,
/// its outcome.
#[derive(Default)]
struct Replay {
    connecting: Option<ShimMessage>,
    authenticated: bool,
    /// `Quiet` while nothing arrives.
    quiet: Option<ShimMessage>,
    /// `Exited` or `Waiting`.
    outcome: Option<ShimMessage>,
}

struct Shared {
    /// The live connection; also serializes writes with the replay.
    current: Mutex<Option<Arc<PipeConnection>>>,
    replay: Mutex<Replay>,
    /// Set once the first connection said hello; stays set (NativeTerm may
    /// answer and hang up within milliseconds).
    reached: AtomicBool,
    stopped: AtomicBool,
    /// Set whenever a message arrives.
    arrived: Event,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Shared {
    fn send(&self, message: ShimMessage) {
        let current = lock(&self.current);
        remember(&self.replay, &message);
        if let Some(conn) = current.as_ref() {
            if conn.send(&message).is_err() {
                // the reader notices and reconnects
                conn.close();
            }
        }
    }
}

/// A cheap handle for sending from other threads (the console control
/// handler).
#[derive(Clone)]
pub struct LinkSender(Arc<Shared>);

impl LinkSender {
    pub fn send(&self, message: ShimMessage) {
        self.0.send(message);
    }
}

pub struct Link {
    shared: Arc<Shared>,
    inbox: Receiver<AppMessage>,
}

impl Link {
    pub fn start(pipe_name: String, hello: ShimMessage) -> Link {
        let (incoming, inbox) = mpsc::channel::<AppMessage>();
        let shared = Arc::new(Shared {
            current: Mutex::new(None),
            replay: Mutex::new(Replay::default()),
            reached: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
            arrived: Event::new().expect("create event"),
        });
        let reader = Arc::clone(&shared);
        std::thread::spawn(move || loop {
            if reader.stopped.load(Ordering::SeqCst) {
                return;
            }
            let connected = pipe::connect(&pipe_name, RETRY);
            crate::debug::log(format!("pipe connect: {:?}", connected.as_ref().map(|_| ())));
            let Ok(conn) = connected else {
                std::thread::sleep(RETRY);
                continue;
            };
            let conn = Arc::new(conn);
            let ok = {
                let mut current = lock(&reader.current);
                let state = lock(&reader.replay);
                let mut ok = conn.send(&hello).is_ok();
                let login = state.authenticated.then_some(&ShimMessage::Authenticated);
                for m in state.connecting.iter().chain(login).chain(state.quiet.iter()) {
                    ok = ok && conn.send(m).is_ok();
                }
                if let Some(m) = &state.outcome {
                    ok = ok && conn.send(m).is_ok();
                }
                if ok {
                    *current = Some(Arc::clone(&conn));
                    reader.reached.store(true, Ordering::SeqCst);
                }
                ok
            };
            if reader.stopped.load(Ordering::SeqCst) {
                conn.close();
                return;
            }
            // read until the connection ends; a failed replay write doesn't
            // mean nothing is left to read: NativeTerm may have sent a
            // command right before hanging up
            let wait = if ok { READ_WAIT } else { Duration::ZERO };
            loop {
                match conn.recv::<AppMessage>(wait) {
                    Ok(Some(m)) => {
                        if incoming.send(m).is_err() {
                            return;
                        }
                        reader.arrived.set();
                    }
                    Ok(None) if ok => {}
                    _ => break,
                }
            }
            crate::debug::log("pipe connection ended");
            {
                let mut current = lock(&reader.current);
                if current.as_ref().is_some_and(|c| Arc::ptr_eq(c, &conn)) {
                    *current = None;
                }
            }
            drop(conn);
            // NativeTerm restarting, or hanging up on us: don't spin
            std::thread::sleep(RETRY);
        });
        Link { shared, inbox }
    }

    pub fn send(&self, message: ShimMessage) {
        self.shared.send(message);
    }

    pub fn sender(&self) -> LinkSender {
        LinkSender(Arc::clone(&self.shared))
    }

    /// The next message within `timeout` (blocks, doesn't poll).
    pub fn recv(&self, timeout: Duration) -> Option<AppMessage> {
        self.inbox.recv_timeout(timeout).ok()
    }

    /// Everything that has arrived; clears the arrival event first, so a
    /// message arriving meanwhile sets it again.
    pub fn drain(&self) -> Vec<AppMessage> {
        self.shared.arrived.reset();
        self.inbox.try_iter().collect()
    }

    /// Signalled when messages may be waiting (see `drain`).
    pub fn arrival_event(&self) -> &Event {
        &self.shared.arrived
    }

    /// Whether NativeTerm has been reached (now or before), waiting up to
    /// `timeout`. Its answers are in the inbox even if it hung up since.
    pub fn wait_reached(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.shared.reached.load(Ordering::SeqCst) {
                return true;
            }
            std::thread::sleep(TICK);
        }
        self.shared.reached.load(Ordering::SeqCst)
    }
}

impl Drop for Link {
    /// Disconnects at once; the tab is no longer NativeTerm's.
    fn drop(&mut self) {
        self.shared.stopped.store(true, Ordering::SeqCst);
        if let Some(conn) = lock(&self.shared.current).take() {
            conn.close();
        }
    }
}

fn remember(replay: &Mutex<Replay>, message: &ShimMessage) {
    let mut state = lock(replay);
    match message {
        ShimMessage::Connecting { .. } => {
            *state = Replay { connecting: Some(message.clone()), ..Replay::default() };
        }
        ShimMessage::Waiting => {
            *state = Replay { outcome: Some(message.clone()), ..Replay::default() };
        }
        ShimMessage::Exited { .. } => {
            state.quiet = None;
            state.outcome = Some(message.clone());
        }
        ShimMessage::Authenticated => state.authenticated = true,
        ShimMessage::Quiet { .. } => state.quiet = Some(message.clone()),
        ShimMessage::Heard => state.quiet = None,
        _ => {}
    }
}
