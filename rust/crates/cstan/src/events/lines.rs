//! The stream lines, as compact JSON with `v` and `kind` first.
use std::collections::BTreeMap;

use capstan_kernel::areas::feed_cursor::{encode_cursor, CursorEntry};
use capstan_kernel::helpers::clean_text;
use serde_json::{json, Value};

use crate::projects::EndpointState;

fn text(value: Value) -> String {
    value.to_string()
}

pub(super) fn hello(version: &str, host: &str, projects: usize) -> String {
    text(
        json!({"v": 1, "kind": "hello", "version": version, "host": clean_text(host), "projects": projects}),
    )
}

pub(super) fn state_name(state: EndpointState) -> &'static str {
    match state {
        EndpointState::Ready => "ready",
        EndpointState::NoKey => "no_key",
        EndpointState::Silent => "silent",
        EndpointState::Refused => "refused",
        EndpointState::Unsupported => "unsupported",
        EndpointState::Gone => "gone",
    }
}

pub(super) fn project(
    id: &str,
    root: &str,
    state: EndpointState,
    position: Option<(&str, u64)>,
) -> String {
    let (epoch, head) = match position {
        Some((epoch, head)) => (json!(epoch), json!(head)),
        None => (Value::Null, Value::Null),
    };
    text(
        json!({"v": 1, "kind": "project", "projectId": id, "root": root, "state": state_name(state), "epoch": epoch, "head": head}),
    )
}

pub(super) fn snapshot(id: &str, section: &str, key: &str, item: &Value) -> String {
    text(
        json!({"v": 1, "kind": "snapshot", "projectId": id, "section": section, "key": key, "item": item}),
    )
}

pub(super) fn snapshot_end(id: &str, head: u64) -> String {
    text(json!({"v": 1, "kind": "snapshotEnd", "projectId": id, "head": head}))
}

pub(super) fn event(id: &str, event: &Value) -> String {
    text(json!({"v": 1, "kind": "event", "projectId": id, "event": event}))
}

pub(super) fn cursor(entries: &BTreeMap<String, CursorEntry>) -> String {
    text(json!({"v": 1, "kind": "cursor", "cursor": encode_cursor(entries)}))
}

pub(super) fn heartbeat(ts: &str) -> String {
    text(json!({"v": 1, "kind": "heartbeat", "ts": ts}))
}

pub(super) fn resync(id: Option<&str>, reason: &str) -> String {
    text(json!({"v": 1, "kind": "resync", "projectId": id, "reason": reason}))
}

/// An error line; the message is cleaned and cut to 1024 bytes.
pub(super) fn error(code: &str, message: &str) -> String {
    let mut message = clean_text(message);
    if message.len() > 1024 {
        let mut end = 1024;
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        message.truncate(end);
    }
    text(json!({"v": 1, "kind": "error", "code": code, "message": message}))
}
