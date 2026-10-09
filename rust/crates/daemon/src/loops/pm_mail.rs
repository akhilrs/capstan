//! What the PM's mail looks like to the operator (src/pm-mail.ts): how many messages wait, which is the oldest and
//! whether it has waited too long. Pure, over message records as JSON.

use capstan_kernel::areas::messaging::parse_iso_ms;
use serde_json::Value;

/// Queued, deferred, sent or unacked: the PM has not acknowledged it yet.
pub const PENDING_STATES: [&str; 4] = ["queued", "deferred", "sent", "unacked"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PmMailSummary {
    pub pending: usize,
    pub oldest_message_id: Option<String>,
    pub oldest_queued_at: Option<String>,
    pub oldest_age_seconds: i64,
    pub stale: bool,
}

/// `Date.parse` of a stored time; None for text that is not one (`NaN`).
pub fn date_parse(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    let shaped = bytes.len() >= 20
        && bytes.get(4) == Some(&b'-')
        && bytes.get(7) == Some(&b'-')
        && bytes.get(10) == Some(&b'T');
    shaped.then(|| parse_iso_ms(text))
}

/// `pmMailSummary`. The oldest is the one queued first.
pub fn pm_mail_summary(messages: &[Value], now_ms: i64, stale_seconds: i64) -> PmMailSummary {
    let mut pending = 0usize;
    let mut oldest: Option<(&Value, Option<i64>)> = None;
    for message in messages {
        let state = message["state"].as_str().unwrap_or("");
        if !PENDING_STATES.contains(&state) {
            continue;
        }
        pending += 1;
        let queued = message["queuedAt"].as_str().and_then(date_parse);
        let replace = match &oldest {
            None => true,
            Some((_, current)) => {
                matches!((queued, current), (Some(q), Some(c)) if q < *c) || current.is_none()
            }
        };
        if replace {
            oldest = Some((message, queued));
        }
    }
    let Some((message, queued)) = oldest else {
        return PmMailSummary {
            pending: 0,
            oldest_message_id: None,
            oldest_queued_at: None,
            oldest_age_seconds: 0,
            stale: false,
        };
    };
    let age = queued.map_or(0, |q| ((now_ms - q) / 1000).max(0));
    PmMailSummary {
        pending,
        oldest_message_id: message["messageId"].as_str().map(str::to_string),
        oldest_queued_at: message["queuedAt"].as_str().map(str::to_string),
        oldest_age_seconds: age,
        stale: age >= stale_seconds,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn counts_pending_and_names_the_oldest() {
        let messages = vec![
            json!({"messageId": "b", "state": "queued", "queuedAt": "2026-01-01T00:10:00.000Z"}),
            json!({"messageId": "a", "state": "sent", "queuedAt": "2026-01-01T00:00:00.000Z"}),
            json!({"messageId": "c", "state": "acked", "queuedAt": "2025-01-01T00:00:00.000Z"}),
        ];
        let now = parse_iso_ms("2026-01-01T00:20:00.000Z");
        let summary = pm_mail_summary(&messages, now, 1200);
        assert_eq!(summary.pending, 2);
        assert_eq!(summary.oldest_message_id.as_deref(), Some("a"));
        assert_eq!(summary.oldest_age_seconds, 1200);
        assert!(summary.stale);
        assert!(!pm_mail_summary(&messages, now, 1201).stale);
    }

    #[test]
    fn no_pending_message_is_not_stale() {
        let summary = pm_mail_summary(&[], 0, 0);
        assert_eq!(summary.pending, 0);
        assert!(!summary.stale);
    }
}
