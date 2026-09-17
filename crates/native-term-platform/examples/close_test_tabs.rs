//! Test helper: serve NativeTerm's pipe for a while and tell every shim
//! that says hello to close its tab (cleans up after a failed live test).
//! Only run it while NativeTerm itself isn't running.
//!
//! `cargo run -p native-term-platform --example close_test_tabs [seconds]`

use std::sync::Arc;
use std::time::{Duration, Instant};

use native_term_session::pipe::{self, PipeListener};
use native_term_session::protocol::{AppMessage, ShimMessage};

fn main() {
    let seconds: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(10);
    let mut listener = PipeListener::bind(&pipe::pipe_name().unwrap()).expect("is NativeTerm running?");
    let started = Instant::now();
    std::thread::spawn(move || loop {
        let Ok(conn) = listener.accept() else { return };
        let conn = Arc::new(conn);
        std::thread::spawn(move || {
            let pid = conn.client_pid().unwrap_or(0);
            loop {
                match conn.recv::<ShimMessage>(Duration::from_secs(30)) {
                    Ok(Some(ShimMessage::Hello { role, wt_session, alias, .. })) => {
                        println!("{:>6} ms pid {pid} {role:?} {wt_session:?} {alias:?}: closing", started.elapsed().as_millis());
                        let _ = conn.send(&AppMessage::Welcome { protocol: 1 });
                        let _ = conn.send(&AppMessage::Close);
                    }
                    Ok(Some(other)) => println!("{:>6} ms pid {pid} {other:?}", started.elapsed().as_millis()),
                    Ok(None) => {}
                    Err(e) => {
                        println!("{:>6} ms pid {pid} gone ({e})", started.elapsed().as_millis());
                        return;
                    }
                }
            }
        });
    });
    std::thread::sleep(Duration::from_secs(seconds));
}
