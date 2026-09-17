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
    connected: Arc<AtomicBool>,
}

/// What a newly (re)connected NativeTerm needs to know.
#[derive(Default)]
struct Replay {
    lifecycle: Option<ShimMessage>,
    authenticated: bool,
}

impl Link {
    pub fn start(pipe_name: String, hello: ShimMessage) -> Link {
        let (outbox, outgoing) = mpsc::channel::<ShimMessage>();
        let (incoming, inbox) = mpsc::channel::<AppMessage>();
        let connected = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&connected);
        let replay = Arc::new(Mutex::new(Replay::default()));
        std::thread::spawn(move || loop {
            let Ok(conn) = pipe::connect(&pipe_name, RETRY) else {
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
                if let Some(m) = &state.lifecycle {
                    ok &= conn.send(m).is_ok();
                }
                if state.authenticated {
                    ok &= conn.send(&ShimMessage::Authenticated).is_ok();
                }
            }
            flag.store(ok, Ordering::SeqCst);
            while ok {
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
            flag.store(false, Ordering::SeqCst);
        });
        Link { outbox, inbox, connected }
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

    pub fn wait_connected(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.connected.load(Ordering::SeqCst) {
                return true;
            }
            std::thread::sleep(TICK);
        }
        self.connected.load(Ordering::SeqCst)
    }
}

fn remember(replay: &Mutex<Replay>, message: &ShimMessage) {
    let mut state = replay.lock().unwrap_or_else(|e| e.into_inner());
    match message {
        ShimMessage::Connecting => {
            state.lifecycle = Some(message.clone());
            state.authenticated = false;
        }
        ShimMessage::Exited { .. } => state.lifecycle = Some(message.clone()),
        ShimMessage::Authenticated => state.authenticated = true,
        _ => {}
    }
}
