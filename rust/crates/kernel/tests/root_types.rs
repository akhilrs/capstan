//! The shared merge and push result types: exact JSON, the sorted and cut file list, and the debug membership checks.

use capstan_config::remote::{DOC_PUSH_REASONS, DOC_REFUSAL_CODES};
use capstan_kernel::integrate::{
    printable_path, Preflight, PushOutcome, PushState, Refusal, RootMerge,
};
use capstan_kernel::records::{MAX_CONFLICT_FILES, MAX_CONFLICT_PATH_CHARS};
use serde_json::{json, Value};
use std::collections::BTreeSet;

fn js<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap()
}

fn names(list: &[&str]) -> Vec<Vec<u8>> {
    list.iter().map(|s| s.as_bytes().to_vec()).collect()
}

#[test]
fn preflight_json() {
    assert_eq!(
        js(&Preflight::AlreadyInHead),
        json!({"kind": "alreadyInHead"})
    );
    assert_eq!(
        js(&Preflight::Ready {
            old_head: "ab".into()
        }),
        json!({"kind": "ready", "oldHead": "ab"})
    );
    assert_eq!(
        js(&Preflight::Refused(Refusal::new("wrong_branch", vec![]))),
        json!({"kind": "refused", "code": "wrong_branch", "files": [], "omitted": 0})
    );
}

#[test]
fn root_merge_json() {
    assert_eq!(
        js(&RootMerge::Merged {
            commit: "c1".into()
        }),
        json!({"kind": "merged", "commit": "c1"})
    );
    assert_eq!(
        js(&RootMerge::AlreadyInHead),
        json!({"kind": "alreadyInHead"})
    );
    assert_eq!(
        js(&RootMerge::Refused(Refusal::new(
            "dirty_tree",
            names(&["a"])
        ))),
        json!({"kind": "refused", "code": "dirty_tree", "files": ["a"], "omitted": 0})
    );
    assert_eq!(
        js(&RootMerge::Refused(Refusal::new("wrong_branch", vec![]))),
        json!({"kind": "refused", "code": "wrong_branch", "files": [], "omitted": 0})
    );
    assert_eq!(
        js(&RootMerge::failed("merge_failed", true)),
        json!({"kind": "failed", "reason": "merge_failed", "restored": true})
    );
    assert_eq!(
        js(&RootMerge::failed("timeout", false)),
        json!({"kind": "failed", "reason": "timeout", "restored": false})
    );
}

#[test]
fn push_outcome_json() {
    assert_eq!(js(&PushOutcome::pushed()), json!({"state": "pushed"}));
    assert_eq!(
        js(&PushOutcome::skipped("url_mismatch")),
        json!({"state": "skipped", "reason": "url_mismatch"})
    );
    assert_eq!(
        js(&PushOutcome::failed("rejected")),
        json!({"state": "failed", "reason": "rejected"})
    );
    assert_eq!(PushOutcome::pushed().state, PushState::Pushed);
}

#[test]
fn refusal_sorts_dedups_and_cuts() {
    let mut input: Vec<String> = (0..60).map(|i| format!("f{i:02}")).collect();
    input.extend((0..5).map(|i| format!("f{i:02}")));
    // A deterministic shuffle.
    let len = input.len();
    for i in 0..len {
        input.swap(i, (i * 37 + 11) % len);
    }
    let refusal = Refusal::new(
        "dirty_tree",
        input.iter().map(|s| s.as_bytes().to_vec()).collect(),
    );
    let expected: Vec<String> = (0..MAX_CONFLICT_FILES)
        .map(|i| format!("f{i:02}"))
        .collect();
    assert_eq!(refusal.files(), expected.as_slice());
    assert_eq!(refusal.omitted(), 10);
    assert_eq!(refusal.code(), "dirty_tree");
    assert_eq!(js(&refusal)["omitted"], json!(10));
}

#[test]
fn refusal_escapes_once() {
    let refusal = Refusal::new("dirty_tree", vec![b"a\nb".to_vec(), b"a\nb".to_vec()]);
    assert_eq!(refusal.files(), ["a\\x0ab".to_string()]);
    assert_eq!(refusal.omitted(), 0);
}

#[test]
fn refusal_matches_printable_path() {
    let comma = b"a,b".to_vec();
    let long = vec![b'x'; MAX_CONFLICT_PATH_CHARS + 40];
    let refusal = Refusal::new("dirty_tree", vec![comma.clone(), long.clone()]);
    let mut want = vec![printable_path(&comma), printable_path(&long)];
    want.sort();
    assert_eq!(refusal.files(), want.as_slice());
    assert_ne!(printable_path(&long), String::from_utf8(long).unwrap());
}

#[test]
fn refusal_orders_by_escaped_form() {
    // 0x01 sorts before "!" raw, but its escape "\x01" sorts after "!" ("\\" is 0x5c, "!" is 0x21).
    let raw_first = vec![0x01u8, b'a'];
    let neighbour = b"!a".to_vec();
    assert!(raw_first < neighbour);
    let refusal = Refusal::new("dirty_tree", vec![raw_first.clone(), neighbour.clone()]);
    assert_eq!(
        refusal.files(),
        [printable_path(&neighbour), printable_path(&raw_first)]
    );
}

#[test]
fn refusal_keeps_marker_literals() {
    assert_eq!(
        Refusal::new("wrong_branch", names(&["(unborn HEAD)"])).files(),
        ["(unborn HEAD)"]
    );
    assert_eq!(
        Refusal::new("unsafe_config", names(&["(config unreadable)"])).files(),
        ["(config unreadable)"]
    );
}

fn set(list: &[&str]) -> BTreeSet<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn doc_lists_match_the_contract() {
    let codes = [
        "unsafe_config",
        "invalid_branch",
        "invalid_commit",
        "wrong_branch",
        "no_identity",
        "dirty_tree",
        "merge_conflict",
        "git_too_old",
    ];
    assert_eq!(set(&codes), set(DOC_REFUSAL_CODES));
    let push = [
        "invalid_branch",
        "unsafe_config",
        "no_upstream",
        "mirror",
        "url_mismatch",
        "push_url_rewritten",
        "not_fast_forward",
        "rejected",
        "timeout",
        "unreachable",
    ];
    assert_eq!(set(&push), set(DOC_PUSH_REASONS));
    let _ = RootMerge::failed("merge_failed", false);
    let _ = RootMerge::failed("timeout", false);
}

#[cfg(debug_assertions)]
#[test]
#[should_panic]
fn refusal_new_panics_on_unknown_code() {
    let _ = Refusal::new("nope", vec![]);
}

#[cfg(debug_assertions)]
#[test]
#[should_panic]
fn root_merge_failed_panics_on_unknown_reason() {
    let _ = RootMerge::failed("nope", false);
}

#[cfg(debug_assertions)]
#[test]
#[should_panic]
fn push_outcome_panics_on_unknown_reason() {
    let _ = PushOutcome::skipped("nope");
}

#[cfg(debug_assertions)]
#[test]
#[should_panic]
fn push_outcome_failed_panics_on_unknown_reason() {
    let _ = PushOutcome::failed("nope");
}
