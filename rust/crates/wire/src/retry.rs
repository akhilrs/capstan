//! Opt-in retries for a call: wait out a busy controller, and ask again after a lost reply when that is safe.
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::transport::call_traced;
use crate::{busy_retry_after, AfterSend, Timeout, WireError, NO_LOST_REPLY_RETRY};

/// How a retrying call behaves.
pub struct RetryPolicy<'a> {
    /// The most time all busy waits together may take.
    pub busy_budget: Duration,
    /// The most time the busy waits may reach, counted from the first attempt.
    pub deadline: Duration,
    /// Whether a lost reply is asked again (see `call_retrying`).
    pub lost_reply: bool,
    /// Receives one line per retry.
    pub log_line: &'a dyn Fn(&str),
    /// Waits; replaced in tests.
    pub sleep: &'a dyn Fn(Duration),
}

/// A call that has a response line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Retried {
    pub line: Vec<u8>,
    /// Calls made, the first included.
    pub attempts: u32,
}

/// A call that ended without a response line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetryFailed {
    pub error: WireError,
    pub attempts: u32,
    /// The daemon's boot id changed while the request was in flight.
    pub restarted: bool,
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A request id: pid, a nanosecond clock and a per-process counter, hex, joined by '-'; at most 48 characters.
pub fn new_request_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{:x}-{:x}-{:x}", std::process::id(), nanos, count)
}

fn boot_path(socket: &Path) -> PathBuf {
    let mut name = socket.as_os_str().to_os_string();
    name.push(".boot");
    PathBuf::from(name)
}

fn read_boot(socket: &Path) -> Option<String> {
    let text = std::fs::read_to_string(boot_path(socket)).ok()?;
    let id = text.trim();
    (!id.is_empty()).then(|| id.to_string())
}

/// Up to 20% on top of `base`, from the clock and the counter (no random source is needed for spreading).
fn jittered(base: Duration) -> Duration {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let mixed = nanos
        .wrapping_add(
            COUNTER
                .fetch_add(1, Ordering::Relaxed)
                .wrapping_mul(0x9E37_79B9),
        )
        .wrapping_mul(0x2545_F491_4F6C_DD1D)
        >> 33;
    base + base.mul_f64((mixed % 2001) as f64 / 10_000.0)
}

fn seconds(d: Duration) -> String {
    format!("{:.1}", d.as_secs_f64())
}

/// Calls the daemon, waiting out `controller_busy` answers and, when `policy.lost_reply` is set, asking again after
/// a reply that never came. `frame_for(Some(rid))` builds the request with the id; one id is made for the whole call.
/// A lost reply (the stream closed with no answer byte, or the call timed out) is asked again, with the same id, at
/// most twice (after 500 ms and 1 s), only when the frame carries the id, the command is not in
/// `NO_LOST_REPLY_RETRY`, and `<socket>.boot` was read before the first connect and holds the same id after the
/// failure.
pub fn call_retrying(
    socket: &Path,
    frame_for: &dyn Fn(Option<&str>) -> Vec<u8>,
    timeout: Timeout,
    policy: &RetryPolicy,
    command: &str,
) -> Result<Retried, RetryFailed> {
    let rid = new_request_id();
    // The id exists to let the controller recognise a repeated request after a lost reply; a call that never
    // asks again after one sends none.
    let frame = frame_for(policy.lost_reply.then_some(rid.as_str()));
    let tail = format!(",\"rid\":\"{rid}\"}}\n");
    let carries_rid = frame.ends_with(tail.as_bytes());
    let may_ask_again = policy.lost_reply && carries_rid && !NO_LOST_REPLY_RETRY.contains(&command);
    let boot_before = if may_ask_again {
        read_boot(socket)
    } else {
        None
    };
    let mut spent = Duration::ZERO;
    let mut budget = policy.busy_budget;
    let (mut attempts, mut lost, mut restarted) = (0u32, 0usize, false);
    let base = Path::new("/tmp");
    loop {
        attempts += 1;
        let began = Instant::now();
        let (result, answered) = call_traced(base, socket, &frame, timeout);
        spent += began.elapsed();
        match result {
            Ok(line) => {
                let Some(ms) = busy_retry_after(&line) else {
                    return Ok(Retried { line, attempts });
                };
                let room = policy.deadline.saturating_sub(spent);
                let wait = jittered(Duration::from_millis(ms)).min(budget).min(room);
                if wait.is_zero() {
                    return Ok(Retried { line, attempts });
                }
                budget -= wait;
                spent += wait;
                (policy.log_line)(&format!(
                    "controller busy, retrying in {} s (attempt {})",
                    seconds(wait),
                    attempts + 1
                ));
                (policy.sleep)(wait);
            }
            Err(error) => {
                let lost_reply = matches!(
                    error,
                    WireError::AfterSend(AfterSend::TimedOut)
                        | WireError::AfterSend(AfterSend::Closed)
                ) && !(answered
                    && error == WireError::AfterSend(AfterSend::Closed));
                let boot_after = boot_before.as_ref().and_then(|_| read_boot(socket));
                if let (Some(before), Some(after)) = (&boot_before, &boot_after) {
                    restarted = before != after;
                }
                if !(lost_reply
                    && may_ask_again
                    && lost < 2
                    && boot_before.is_some()
                    && boot_before == boot_after)
                {
                    return Err(RetryFailed {
                        error,
                        attempts,
                        restarted,
                    });
                }
                let pause = Duration::from_millis(if lost == 0 { 500 } else { 1000 });
                lost += 1;
                (policy.log_line)(&format!(
                    "controller did not answer, retrying (attempt {})",
                    attempts + 1
                ));
                (policy.sleep)(pause);
            }
        }
    }
}
