//! The tail: reads the push events the ledger has gained since the last read and hands them to the sender. It never sends.

use crate::deps::KernelHandle;
use crate::loops::driver::DriverLog;
use crate::loops::notifier::Clock;
use capstan_kernel::areas::push_events::{push_events_after, push_head, PushEvent, PushPage};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

/// How often the ledger is read when reads are quick.
pub const BASE_INTERVAL_MS: u64 = 2_000;
/// The longest wait between reads.
pub const MAX_INTERVAL_MS: u64 = 30_000;
/// A read that takes longer than this doubles the wait.
pub const SLOW_READ_MS: i64 = 250;
const PAGE_ROWS: usize = 200;

/// Where push events come from.
pub trait PushSource: Send + Sync {
    /// The sequence of the latest row.
    fn head(&self) -> Result<u64, String>;
    /// The events after `after`.
    fn read(&self, after: u64) -> Result<PushPage, String>;
}

/// The kernel as a source.
pub struct KernelSource(pub KernelHandle);

impl PushSource for KernelSource {
    fn head(&self) -> Result<u64, String> {
        self.0.run(push_head).map_err(|error| error.to_string())
    }

    fn read(&self, after: u64) -> Result<PushPage, String> {
        self.0
            .run(move |core| push_events_after(core, after, PAGE_ROWS))
            .map_err(|error| error.to_string())
    }
}

pub struct Tail {
    source: Arc<dyn PushSource>,
    clock: Clock,
    sink: Box<dyn Fn(&PushEvent) + Send>,
    log: DriverLog,
    cursor: Option<u64>,
    interval_ms: u64,
    failing: bool,
}

impl Tail {
    pub fn new(
        source: Arc<dyn PushSource>,
        clock: Clock,
        log: DriverLog,
        sink: Box<dyn Fn(&PushEvent) + Send>,
    ) -> Self {
        Self {
            source,
            clock,
            sink,
            log,
            cursor: None,
            interval_ms: BASE_INTERVAL_MS,
            failing: false,
        }
    }

    /// The sequence the next read starts after; None until the head was read.
    pub fn cursor(&self) -> Option<u64> {
        self.cursor
    }

    /// One read; returns how long to wait before the next.
    pub fn step(&mut self) -> Duration {
        let started = (self.clock)();
        let result = self.read_once();
        let took = (self.clock)() - started;
        match result {
            Ok(()) => {
                self.failing = false;
                self.interval_ms = if took > SLOW_READ_MS {
                    self.back_off()
                } else {
                    BASE_INTERVAL_MS
                };
            }
            Err(error) => {
                if !self.failing {
                    (self.log)(
                        "ntfy:read_failed",
                        json!({"error": crate::ntfy::redact::clip(&error)}),
                    );
                }
                self.failing = true;
                self.interval_ms = self.back_off();
            }
        }
        Duration::from_millis(self.interval_ms)
    }

    fn back_off(&self) -> u64 {
        (self.interval_ms * 2).min(MAX_INTERVAL_MS)
    }

    fn read_once(&mut self) -> Result<(), String> {
        let cursor = match self.cursor {
            Some(cursor) => cursor,
            None => {
                let head = self.source.head()?;
                self.cursor = Some(head);
                head
            }
        };
        let page = self.source.read(cursor)?;
        for event in &page.events {
            (self.sink)(event);
        }
        self.cursor = Some(page.scanned.max(cursor));
        Ok(())
    }
}
