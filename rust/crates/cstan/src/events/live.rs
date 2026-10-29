//! The real sources: the controller sockets, the process list and the system clock.
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use capstan_client::{FeedCall, Response};
use capstan_wire::js;
use serde_json::Value;

use super::{Clock, Deps, FeedFailure, FeedSource, Limits, Processes};
use crate::controllers::{self, Proc};
use crate::projects::{call_within, Credential, Live};

/// How long one call may take in all.
const CALL_TIMEOUT_MS: u64 = 2_000;

pub(super) fn deps(cwd: PathBuf, err: Box<dyn Write + Send>) -> Deps {
    Deps {
        feed: Box::new(LiveFeed),
        probe: Box::new(Live),
        procs: Box::new(LiveProcs),
        clock: Box::new(SystemClock),
        err,
        limits: Limits::default(),
        cwd,
    }
}

pub(super) struct LiveFeed;

impl FeedSource for LiveFeed {
    fn call(&self, root: &Path, key: &Credential, call: &FeedCall) -> Result<Value, FeedFailure> {
        match call_within(root, key, &call.args(), CALL_TIMEOUT_MS) {
            Some(Ok(Response::Ok {
                result: Some(result),
            })) => {
                let text = js::stringify(&result, 0).to_utf8_lossy();
                match serde_json::from_str::<Value>(&text) {
                    Ok(value) if value.is_object() => Ok(value),
                    _ => Err(FeedFailure::NoAnswer),
                }
            }
            Some(Ok(Response::Refused { code, message })) => Err(FeedFailure::Refused {
                code: code.to_utf8_lossy(),
                message: message.to_utf8_lossy(),
            }),
            _ => Err(FeedFailure::NoAnswer),
        }
    }
}

pub(super) struct LiveProcs;

impl Processes for LiveProcs {
    fn list(&self) -> Vec<Proc> {
        controllers::processes()
    }
}

pub(super) struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    fn sleep_ms(&self, ms: u64) {
        std::thread::sleep(Duration::from_millis(ms));
    }
}
