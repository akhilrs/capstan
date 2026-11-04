//! The ntfy sender: pushes the events that need the user to an ntfy server.
//!
//! Three parts run on their own threads, all joined by the loops' stop: the tail reads push events from the kernel and
//! queues them, the worker sends them (it never calls the kernel), and a `notify` test goes through the same transport.
//! With no `[ntfy]` section or `enabled = false` nothing is spawned and `status` has no `ntfy` member.

pub mod message;
pub mod redact;
pub mod sender;
pub mod tail;
pub mod transport;

use crate::deps::Deps;
use crate::loops::driver::DriverLog;
use crate::loops::notifier::Clock;
use crate::loops::StopFlag;
use capstan_config::{effective_prefix, RoleConfig};
pub use sender::{SenderHandle, SenderOptions, TestAnswer, TestBusy};
use std::sync::Arc;
use std::thread::JoinHandle;
pub use tail::{KernelSource, PushSource, Tail};
pub use transport::{CurlTransport, NtfyTransport, PublishRequest, TransportError};

/// Starts the sender when `config` enables it; returns its threads. The handle is published on `deps` before they run, so
/// `status` and `notify` see the sender from its first moment.
pub fn start(
    deps: &Deps,
    config: &RoleConfig,
    stop: &StopFlag,
    clock: &Clock,
    log: &DriverLog,
    transport: Option<Arc<dyn NtfyTransport>>,
) -> Vec<JoinHandle<()>> {
    if !config.ntfy.enabled {
        return Vec::new();
    }
    let transport = transport.unwrap_or_else(|| Arc::new(CurlTransport::new(&config.ntfy)));
    let source = Arc::new(KernelSource(deps.kernel.clone()));
    let handle = SenderHandle::new(SenderOptions {
        prefix: effective_prefix(&config.ntfy, config.project_name.as_deref()),
        ntfy: config.ntfy.clone(),
        transport,
        clock: Arc::clone(clock),
        stop: stop.clone(),
        log: Arc::clone(log),
        status_path: Some(deps.options.state_directory.join(sender::STATUS_FILE)),
        env: Arc::new(|name| std::env::var(name).ok()),
    });
    start_threads(&handle, source, clock, log, stop, || deps.set_ntfy(&handle))
}

/// The threads of an already built sender over `source`; `publish` runs before they start.
pub fn start_threads(
    handle: &Arc<SenderHandle>,
    source: Arc<dyn PushSource>,
    clock: &Clock,
    log: &DriverLog,
    stop: &StopFlag,
    publish: impl FnOnce(),
) -> Vec<JoinHandle<()>> {
    publish();
    let mut handles = Vec::new();
    let worker = Arc::clone(handle);
    handles.extend(crate::loops::spawn("ntfy-send", move || {
        worker.run_worker()
    }));
    let sender = Arc::clone(handle);
    let mut tail = Tail::new(
        source,
        Arc::clone(clock),
        Arc::clone(log),
        Box::new(move |event| sender.enqueue(event)),
    );
    let flag = stop.clone();
    handles.extend(crate::loops::spawn("ntfy-tail", move || {
        while !flag.stopped() {
            let wait = tail.step();
            if flag.sleep(wait) {
                break;
            }
        }
    }));
    handles
}
