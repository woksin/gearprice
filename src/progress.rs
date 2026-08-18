//! A spinner on stderr, for the seconds a price report spends probing.
//!
//! Measuring a large market takes dozens of small requests. Without a sign of life that
//! reads as a hang; with one it reads as work. It writes to stderr only, so a piped
//! `--format json` run stays clean.

use std::io::{self, IsTerminal, Write};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub struct Progress {
    enabled: bool,
    message: Arc<Mutex<String>>,
    stopped: Arc<AtomicBool>,
    /// Requests sent so far, shared with the client.
    ///
    /// A spinner says the program has not hung. A count says what it is doing and roughly
    /// how much is left, which over a half-minute wait is the difference between patience
    /// and reaching for ctrl-C.
    counter: Arc<AtomicU32>,
    worker: Option<JoinHandle<()>>,
}

impl Progress {
    pub fn new(disabled: bool) -> Self {
        let enabled = !disabled
            && io::stderr().is_terminal()
            && std::env::var_os("CI").is_none()
            && std::env::var_os("GEARPRICE_NO_PROGRESS").is_none();
        let message = Arc::new(Mutex::new("Starting".to_string()));
        let stopped = Arc::new(AtomicBool::new(false));
        let counter = Arc::new(AtomicU32::new(0));

        let worker = enabled.then(|| {
            let message = Arc::clone(&message);
            let stopped = Arc::clone(&stopped);
            let counter = Arc::clone(&counter);
            thread::spawn(move || {
                let started = Instant::now();
                let mut stderr = io::stderr().lock();
                let mut frame = 0;
                while !stopped.load(Ordering::Relaxed) {
                    let current = message
                        .lock()
                        .map(|value| value.clone())
                        .unwrap_or_else(|_| "Working".to_string());
                    let sent = counter.load(Ordering::Relaxed);
                    let asked = if sent == 0 {
                        String::new()
                    } else {
                        format!(" · {sent} lookups")
                    };
                    let _ = write!(
                        stderr,
                        "\r\x1b[2K  \x1b[36m{}\x1b[0m {current}  \x1b[2m{:.0}s{asked}\x1b[0m",
                        FRAMES[frame % FRAMES.len()],
                        started.elapsed().as_secs_f64()
                    );
                    let _ = stderr.flush();
                    frame += 1;
                    thread::sleep(Duration::from_millis(80));
                }
            })
        });

        Self {
            enabled,
            message,
            stopped,
            counter,
            worker,
        }
    }

    /// The counter the client bumps as it sends requests.
    pub fn counter(&self) -> Arc<AtomicU32> {
        Arc::clone(&self.counter)
    }

    pub fn set(&self, message: impl Into<String>) {
        if !self.enabled {
            return;
        }
        if let Ok(mut current) = self.message.lock() {
            *current = message.into();
        }
    }

    /// Stops the spinner and wipes its line, so report output starts on a clean row.
    pub fn finish(&mut self) {
        if !self.enabled {
            return;
        }
        self.stopped.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        let mut stderr = io::stderr().lock();
        let _ = write!(stderr, "\r\x1b[2K");
        let _ = stderr.flush();
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        self.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_disabled_spinner_starts_no_thread_and_stops_cleanly() {
        let mut progress = Progress::new(true);
        assert!(!progress.enabled);
        assert!(progress.worker.is_none());
        // The counter is handed out whether or not anything is drawing it, so the client
        // never has to know if a spinner is running.
        progress.counter().store(3, Ordering::Relaxed);
        assert_eq!(3, progress.counter().load(Ordering::Relaxed));
        progress.set("anything");
        progress.finish();
        // Finishing twice is safe, which matters because Drop finishes it again.
        progress.finish();
    }
}
