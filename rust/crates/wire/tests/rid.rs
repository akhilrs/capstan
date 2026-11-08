//! `frame_with_rid` against `frame`, and `busy_retry_after`.
use capstan_wire::{busy_retry_after, frame, frame_with_rid, MAX_FRAME_BYTES};

#[test]
fn frame_with_rid_is_frame_plus_a_last_member() {
    let args = vec!["--json".to_string(), "é\"x".to_string()];
    let plain = frame("tok", "status", &args).unwrap();
    let with = frame_with_rid("tok", "status", &args, "ab-12").unwrap();
    let body = &plain[..plain.len() - 2]; // drop `}` and the newline
    assert!(with.starts_with(body));
    assert_eq!(&with[body.len()..], b",\"rid\":\"ab-12\"}\n");
}

#[test]
fn frame_with_rid_has_the_same_limit() {
    let big = vec!["x".repeat(MAX_FRAME_BYTES - 80)];
    assert!(frame("t", "send", &big).is_ok());
    assert!(frame_with_rid("t", "send", &big, &"r".repeat(48)).is_err());
}

#[test]
fn busy_retry_after_cases() {
    let some = |s: &str| busy_retry_after(s.as_bytes());
    assert_eq!(
        some(r#"{"ok":false,"code":"controller_busy","retryAfterMs":250}"#),
        Some(250)
    );
    assert_eq!(
        some(r#"{"ok":false,"code":"controller_busy","message":"m"}"#),
        Some(500)
    );
    assert_eq!(
        some(r#"{"ok":false,"code":"controller_busy","retryAfterMs":"x"}"#),
        Some(500)
    );
    assert_eq!(some(r#"{"ok":false,"code":"busy","retryAfterMs":250}"#), None);
    assert_eq!(some(r#"{"ok":true,"code":"controller_busy"}"#), None);
    assert_eq!(some(r#"{"ok":false,"code":"invalid_request"}"#), None);
    assert_eq!(some("not json"), None);
    assert_eq!(busy_retry_after(&[0xff, 0xfe]), None);
}
