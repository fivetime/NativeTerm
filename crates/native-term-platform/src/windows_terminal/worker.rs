//! UIA calls on a worker thread with a deadline. A call into a Terminal
//! that hangs after the `WM_NULL` probe never returns; the caller gives up
//! at the deadline, the stuck thread is abandoned (never joined), and the
//! next call gets a fresh worker.

use std::sync::mpsc::{self, Sender};
use std::time::Duration;

use uiautomation::UIAutomation;

type Job = Box<dyn FnOnce(&UIAutomation) + Send>;

pub struct Worker {
    jobs: Option<Sender<Job>>,
    abandoned: usize,
}

impl Default for Worker {
    fn default() -> Self {
        Worker::new()
    }
}

impl Worker {
    pub fn new() -> Worker {
        Worker { jobs: None, abandoned: 0 }
    }

    /// How many stuck workers were left behind so far.
    pub fn abandoned(&self) -> usize {
        self.abandoned
    }

    fn spawn() -> Sender<Job> {
        let (jobs, incoming) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("nativeterm-uia".into())
            .spawn(move || {
                let Ok(automation) = UIAutomation::new() else { return };
                // ends when the sender is dropped (worker replaced or dropped)
                while let Ok(job) = incoming.recv() {
                    job(&automation);
                }
            })
            .expect("spawn UIA worker");
        jobs
    }

    /// Run `f` with the worker's automation object; `None` on timeout or if
    /// the worker couldn't start.
    pub fn run<R, F>(&mut self, timeout: Duration, f: F) -> Option<R>
    where
        R: Send + 'static,
        F: FnOnce(&UIAutomation) -> R + Send + 'static,
    {
        let jobs = self.jobs.get_or_insert_with(Worker::spawn);
        let (reply, result) = mpsc::sync_channel(1);
        let job: Job = Box::new(move |automation| {
            let _ = reply.send(f(automation));
        });
        if jobs.send(job).is_err() {
            // the thread ended (UIA unavailable); try a new one next time
            self.jobs = None;
            return None;
        }
        match result.recv_timeout(timeout) {
            Ok(value) => Some(value),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.jobs = None;
                self.abandoned += 1;
                None
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                self.jobs = None;
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_and_survives_a_stuck_call() {
        let mut worker = Worker::new();
        assert_eq!(worker.run(Duration::from_secs(5), |_| 1), Some(1));
        let started = std::time::Instant::now();
        assert_eq!(worker.run(Duration::from_millis(100), |_| std::thread::sleep(Duration::from_secs(3))), None);
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(worker.abandoned(), 1);
        assert_eq!(worker.run(Duration::from_secs(5), |_| 2), Some(2), "a fresh worker");
    }
}
