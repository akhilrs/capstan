//! Tells the operator that something needs them (src/notifier.ts): a Herdr notification and a passive fallback the
//! `cstan status --watch` pane shows with a bell. Every channel attempt leaves one line in notifications.jsonl, so a
//! failed channel is never silent.

use capstan_herdr::api::{AdapterError, NotifierAdapter};
use serde_json::{json, Map, Value};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::PathBuf;
use std::sync::Arc;

pub const NOTIFICATION_LOG_MAX_BYTES: u64 = 5 * 1024 * 1024;
pub const NOTIFICATION_ERROR_MAX_CHARS: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotificationKind {
    PmMessage,
    InputCleared,
    DeliveryStuck,
    PmStale,
}

impl NotificationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PmMessage => "pm_message",
            Self::InputCleared => "input_cleared",
            Self::DeliveryStuck => "delivery_stuck",
            Self::PmStale => "pm_stale",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::PmMessage => "Capstan: PM message waiting",
            Self::InputCleared => "Capstan: input line cleared",
            Self::DeliveryStuck => "Capstan: delivery stuck",
            Self::PmStale => "Capstan: PM mail is stale",
        }
    }
}

#[derive(Clone, Debug)]
pub struct NotificationRequest {
    pub kind: NotificationKind,
    pub message_id: String,
    pub recipient_agent_id: String,
    pub repeat: bool,
    /// Characters cleared (`input_cleared`) or the stuck reason (`delivery_stuck`).
    pub detail: Option<String>,
}

impl NotificationRequest {
    fn body(&self) -> String {
        let detail = self.detail.as_deref();
        match self.kind {
            NotificationKind::PmMessage => format!(
                "Message {} for the PM is waiting{}",
                self.message_id,
                if self.repeat { " (reminder)" } else { "" }
            ),
            NotificationKind::InputCleared => format!(
                "Cleared {} characters typed in {}'s input line before message {}",
                detail.unwrap_or("some"),
                self.recipient_agent_id,
                self.message_id
            ),
            NotificationKind::PmStale => format!(
                "{} for the PM has waited too long (oldest: {})",
                detail.unwrap_or("A message"),
                self.message_id
            ),
            NotificationKind::DeliveryStuck => format!(
                "Message {} for {} is stuck: {}",
                self.message_id,
                self.recipient_agent_id,
                detail.unwrap_or("unknown reason")
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelFailureReason {
    NotShown,
    CommandFailed,
    InvalidText,
    Timeout,
}

impl ChannelFailureReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotShown => "not_shown",
            Self::CommandFailed => "command_failed",
            Self::InvalidText => "invalid_text",
            Self::Timeout => "timeout",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelResult {
    /// `herdr` or `fallback`.
    pub channel: &'static str,
    pub ok: bool,
    pub reason: Option<ChannelFailureReason>,
    /// The sanitized error message of a failed channel, at most NOTIFICATION_ERROR_MAX_CHARS characters.
    pub error: Option<String>,
}

/// Names the failure from the error's class and code; a Herdr answer of shown:false carries the code
/// `notification_not_shown`.
fn failure_reason(error: &AdapterError) -> ChannelFailureReason {
    match error.herdr_code() {
        Some("notification_not_shown") => ChannelFailureReason::NotShown,
        Some("timeout") => ChannelFailureReason::Timeout,
        _ if matches!(error, AdapterError::InvalidArgument(_)) => ChannelFailureReason::InvalidText,
        _ => ChannelFailureReason::CommandFailed,
    }
}

fn is_control(c: char) -> bool {
    matches!(c, '\u{0}'..='\u{1f}' | '\u{7f}'..='\u{9f}')
}

/// The error's message with control characters and runs of white space made single spaces, cut to
/// NOTIFICATION_ERROR_MAX_CHARS UTF-16 units.
fn sanitized_error(error: &AdapterError) -> String {
    let mut spaced = String::new();
    let mut in_run = false;
    for c in error.message().chars() {
        if is_control(c) || c.is_whitespace() || c == '\u{feff}' {
            if !in_run {
                spaced.push(' ');
            }
            in_run = true;
        } else {
            spaced.push(c);
            in_run = false;
        }
    }
    let trimmed = spaced.trim();
    let mut out = String::new();
    let mut units = 0usize;
    for c in trimmed.chars() {
        units += c.len_utf16();
        if units > NOTIFICATION_ERROR_MAX_CHARS {
            break;
        }
        out.push(c);
    }
    out
}

pub type NotifierLog = Arc<dyn Fn(&str, Value) + Send + Sync>;
pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

pub struct Notifier {
    adapter: Arc<dyn NotifierAdapter>,
    herdr: bool,
    fallback: bool,
    record_path: PathBuf,
    now: Clock,
    log: NotifierLog,
}

impl Notifier {
    pub fn new(
        adapter: Arc<dyn NotifierAdapter>,
        herdr: bool,
        fallback: bool,
        record_path: PathBuf,
        now: Clock,
        log: NotifierLog,
    ) -> Self {
        Self {
            adapter,
            herdr,
            fallback,
            record_path,
            now,
            log,
        }
    }

    /// Attempts every enabled channel; never fails.
    pub fn send(&self, request: &NotificationRequest) -> Vec<ChannelResult> {
        let mut results = Vec::new();
        if self.herdr {
            match self.adapter.notify(request.kind.title(), &request.body()) {
                Ok(()) => results.push(ChannelResult {
                    channel: "herdr",
                    ok: true,
                    reason: None,
                    error: None,
                }),
                Err(error) => {
                    let reason = failure_reason(&error);
                    let message = sanitized_error(&error);
                    (self.log)(
                        "notification_channel_failed",
                        json!({
                            "channel": "herdr",
                            "messageId": request.message_id,
                            "reason": reason.as_str(),
                            "error": error.name(),
                        }),
                    );
                    results.push(ChannelResult {
                        channel: "herdr",
                        ok: false,
                        reason: Some(reason),
                        error: (!message.is_empty()).then_some(message),
                    });
                }
            }
        }
        // Passive: the ledger record is what the watch pane reads.
        if self.fallback {
            results.push(ChannelResult {
                channel: "fallback",
                ok: true,
                reason: None,
                error: None,
            });
        }
        results
    }

    /// Appends one line per channel; never fails. `recorded` says the ledger holds a record of the event.
    pub fn write(&self, request: &NotificationRequest, results: &[ChannelResult], recorded: bool) {
        if let Err(error) = self.append(request, results, recorded) {
            (self.log)(
                "notification_record_failed",
                json!({"messageId": request.message_id, "error": error}),
            );
        }
    }

    fn append(
        &self,
        request: &NotificationRequest,
        results: &[ChannelResult],
        recorded: bool,
    ) -> Result<(), &'static str> {
        match std::fs::metadata(&self.record_path) {
            Ok(meta) if meta.len() > NOTIFICATION_LOG_MAX_BYTES => {
                let mut rotated = self.record_path.clone().into_os_string();
                rotated.push(".1");
                std::fs::rename(&self.record_path, rotated).map_err(|_| "Error")?;
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err("Error"),
        }
        let mut file = open_log(&self.record_path).map_err(|_| "Error")?;
        let ts = capstan_ledger::iso_from_millis((self.now)());
        for result in results {
            let mut line = Map::new();
            line.insert("ts".into(), json!(ts));
            line.insert("messageId".into(), json!(request.message_id));
            line.insert("channel".into(), json!(result.channel));
            line.insert("ok".into(), json!(result.ok));
            if let Some(reason) = result.reason {
                line.insert("reason".into(), json!(reason.as_str()));
            }
            if let Some(error) = &result.error {
                line.insert("error".into(), json!(error));
            }
            line.insert("kind".into(), json!(request.kind.as_str()));
            line.insert("repeat".into(), json!(request.repeat));
            line.insert("recorded".into(), json!(recorded));
            let text = Value::Object(line).to_string();
            file.write_all(format!("{text}\n").as_bytes())
                .map_err(|_| "Error")?;
        }
        Ok(())
    }
}

/// `openDaemonLog`: appends to a file private to the user (created 0600, never through a symlink).
fn open_log(path: &PathBuf) -> std::io::Result<std::fs::File> {
    let file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let meta = file.metadata()?;
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    if !meta.is_file() || meta.mode() & 0o777 != 0o600 || meta.uid() != uid {
        return Err(std::io::Error::other(
            "daemon log must be a regular file owned by the current user with mode 0600",
        ));
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use capstan_herdr::api::HerdrError;
    use std::sync::Mutex;

    struct Scripted(
        Mutex<Vec<Result<(), AdapterError>>>,
        Mutex<Vec<(String, String)>>,
    );

    impl NotifierAdapter for Scripted {
        fn notify(&self, title: &str, body: &str) -> Result<(), AdapterError> {
            self.1.lock().unwrap().push((title.into(), body.into()));
            self.0.lock().unwrap().remove(0)
        }
    }

    fn request(kind: NotificationKind) -> NotificationRequest {
        NotificationRequest {
            kind,
            message_id: "m1".into(),
            recipient_agent_id: "dev-1".into(),
            repeat: true,
            detail: Some("pane_mismatch".into()),
        }
    }

    #[test]
    fn a_failed_herdr_channel_is_named_and_the_fallback_still_answers() {
        let adapter = Arc::new(Scripted(
            Mutex::new(vec![
                Ok(()),
                Err(AdapterError::Herdr(HerdrError::new(
                    "notification_not_shown",
                    "line one\n\tline  two",
                ))),
            ]),
            Mutex::new(Vec::new()),
        ));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notifications.jsonl");
        let notifier = Notifier::new(
            adapter.clone(),
            true,
            true,
            path.clone(),
            Arc::new(|| 0),
            Arc::new(|_, _| {}),
        );
        let first = notifier.send(&request(NotificationKind::PmMessage));
        assert!(first.iter().all(|r| r.ok));
        let second = notifier.send(&request(NotificationKind::DeliveryStuck));
        assert_eq!(second[0].reason, Some(ChannelFailureReason::NotShown));
        assert_eq!(second[0].error.as_deref(), Some("line one line two"));
        assert!(second[1].ok);
        notifier.write(&request(NotificationKind::DeliveryStuck), &second, false);
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["reason"], "not_shown");
        assert_eq!(lines[0]["recorded"], false);
        assert_eq!(lines[1]["channel"], "fallback");
        let sent = adapter.1.lock().unwrap().clone();
        assert_eq!(sent[0].1, "Message m1 for the PM is waiting (reminder)");
        assert_eq!(sent[1].1, "Message m1 for dev-1 is stuck: pane_mismatch");
    }
}
