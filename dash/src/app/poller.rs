//! The poll loop: one request in flight, change detection by hash, pause, forced polls and back-off.
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::client::ClientError;
use crate::view::Link;

pub const BACKOFF_MS: [u64; 3] = [1000, 2000, 5000];
pub const MIN_INTERVAL_SECONDS: u32 = 1;
pub const MAX_INTERVAL_SECONDS: u32 = 60;

/// Delay before the next try after `failures` in a row; the poll interval when there are none.
pub fn next_delay_ms(failures: u32, interval_ms: u64, backoff: &[u64]) -> u64 {
    if failures == 0 {
        return interval_ms;
    }
    backoff[(failures as usize).min(backoff.len()) - 1]
}

/// The poll interval one `-` (`faster`) or `+` key press away: steps of 1 s up to 10 s, then 5 s, kept within 1 to 60.
pub fn step_interval(seconds: u32, faster: bool) -> u32 {
    let step = if (faster && seconds <= 10) || (!faster && seconds < 10) {
        1
    } else {
        5
    };
    let next = if faster {
        seconds.saturating_sub(step)
    } else {
        seconds + step
    };
    next.clamp(MIN_INTERVAL_SECONDS, MAX_INTERVAL_SECONDS)
}

/// A digest of a status; equal statuses give equal digests.
pub fn status_hash(status: &Value) -> u64 {
    let mut hasher = DefaultHasher::new();
    status.to_string().hash(&mut hasher);
    hasher.finish()
}

/// What one poll reports.
#[derive(Clone, Debug, PartialEq)]
pub enum PollEvent {
    Status { status: Value, changed: bool },
    Failed { link: Link },
}

#[derive(Debug)]
struct Control {
    stopped: bool,
    paused: bool,
    forced: bool,
    interval_ms: u64,
    /// Counts the reasons to cut a sleep short: a forced poll, a new interval, stop.
    wake: u64,
}

struct Shared {
    control: Mutex<Control>,
    changed: Condvar,
}

/// Handle to a poller thread.
pub struct Poller {
    shared: Arc<Shared>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

fn lock(shared: &Shared) -> MutexGuard<'_, Control> {
    shared.control.lock().unwrap_or_else(|e| e.into_inner())
}

impl Poller {
    /// Starts polling at once. `fetch` runs on the poller thread, never twice at the same time.
    pub fn spawn<F, E>(interval_ms: u64, backoff: Vec<u64>, mut fetch: F, mut emit: E) -> Poller
    where
        F: FnMut() -> Result<Value, ClientError> + Send + 'static,
        E: FnMut(PollEvent) + Send + 'static,
    {
        let shared = Arc::new(Shared {
            control: Mutex::new(Control {
                stopped: false,
                paused: false,
                forced: false,
                interval_ms,
                wake: 0,
            }),
            changed: Condvar::new(),
        });
        let inner = Arc::clone(&shared);
        let thread = std::thread::spawn(move || {
            let mut failures = 0u32;
            let mut hash: Option<u64> = None;
            loop {
                let poll = {
                    let mut control = lock(&inner);
                    if control.stopped {
                        return;
                    }
                    let poll = !control.paused || control.forced;
                    control.forced = false;
                    poll
                };
                if poll {
                    match fetch() {
                        Ok(status) => {
                            failures = 0;
                            let next = status_hash(&status);
                            let changed = hash != Some(next);
                            hash = Some(next);
                            emit(PollEvent::Status { status, changed });
                        }
                        Err(error) => {
                            failures += 1;
                            emit(PollEvent::Failed { link: error.link() });
                        }
                    }
                }
                // Sleep; a forced poll or a stop ends it, a new interval restarts it.
                let mut control = lock(&inner);
                loop {
                    if control.stopped || control.forced {
                        break;
                    }
                    let seen = control.wake;
                    let deadline = Instant::now()
                        + Duration::from_millis(next_delay_ms(
                            failures,
                            control.interval_ms,
                            &backoff,
                        ));
                    let restarted = loop {
                        if control.stopped || control.forced {
                            break false;
                        }
                        if control.wake != seen {
                            break true;
                        }
                        let Some(left) = deadline
                            .checked_duration_since(Instant::now())
                            .filter(|d| !d.is_zero())
                        else {
                            break false;
                        };
                        control = inner
                            .changed
                            .wait_timeout(control, left)
                            .unwrap_or_else(|e| e.into_inner())
                            .0;
                    };
                    if !restarted {
                        break;
                    }
                }
            }
        });
        Poller {
            shared,
            thread: Mutex::new(Some(thread)),
        }
    }

    fn update(&self, change: impl FnOnce(&mut Control)) {
        let mut control = lock(&self.shared);
        change(&mut control);
        self.shared.changed.notify_all();
    }

    /// Ends the loop; a request in flight still finishes.
    pub fn stop(&self) {
        self.update(|c| {
            c.stopped = true;
            c.wake += 1;
        });
    }

    /// Polls now, even when paused.
    pub fn poll_now(&self) {
        self.update(|c| {
            c.forced = true;
            c.wake += 1;
        });
    }

    pub fn set_paused(&self, paused: bool) {
        self.update(|c| c.paused = paused);
    }

    /// Changes the interval; the sleep in progress restarts with it.
    pub fn set_interval_ms(&self, interval_ms: u64) {
        self.update(|c| {
            c.interval_ms = interval_ms;
            c.wake += 1;
        });
    }

    /// Stops and waits for the thread.
    pub fn join(&self) {
        self.stop();
        let handle = self.thread.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(handle) = handle {
            let _ = handle.join();
        }
    }
}

impl Drop for Poller {
    fn drop(&mut self) {
        self.stop();
    }
}
