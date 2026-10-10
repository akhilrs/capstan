//! The PM restart summary's budget and truncation rules (test/panes.test.ts): the summary always fits 32 KiB, a body is cut
//! by characters (not UTF-16 units) at 2000 and marked, a very large task brief is a marked preview, a body that was
//! already cut is not cut again, a body that merely ends in the marker is still cut, and a joined character is not split.

use crate::fixture::Fixture;
use serde_json::Value;

const MARKER: &str = "[truncated]";

fn chars(text: &str) -> usize {
    text.chars().count()
}

fn messages(summary: &Value) -> Vec<String> {
    summary["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["body"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn a_summary_with_sixty_messages_keeps_fifty_and_cuts_the_first_long_body() {
    let f = Fixture::new();
    for i in 0..60 {
        f.tell_pm(&if i == 0 {
            "é".repeat(3000)
        } else {
            format!("message {i}")
        });
    }
    let summary = f.restart_pm();
    let bodies = messages(&summary);
    assert_eq!(bodies.len(), 50);
    assert_eq!(summary["truncated"], true);
    assert!(
        bodies[0].ends_with(MARKER),
        "{}",
        &bodies[0][bodies[0].len() - 20..]
    );
}

#[test]
fn a_summary_always_fits_its_budget_however_much_there_is() {
    let f = Fixture::new();
    for i in 0..40 {
        f.tell_pm(&format!("{i} {}", "x".repeat(2400)));
    }
    let summary = f.restart_pm();
    assert!(
        serde_json::to_string(&summary).unwrap().len() <= 32 * 1024,
        "{} bytes",
        serde_json::to_string(&summary).unwrap().len()
    );
    assert_eq!(summary["truncated"], true);
    assert!(
        messages(&summary).len() < 40,
        "the oldest are dropped to fit"
    );
}

#[test]
fn a_body_is_cut_by_characters_not_utf16_units() {
    let f = Fixture::new();
    // 2000 astral characters are 4000 UTF-16 units: exactly at the limit, not over it.
    f.tell_pm(&"😀".repeat(2000));
    f.tell_pm(&"😀".repeat(2001));
    let bodies = messages(&f.restart_pm());
    assert_eq!(
        bodies[0],
        "😀".repeat(2000),
        "a body at the limit is kept whole"
    );
    assert_eq!(bodies[1], format!("{}{MARKER}", "😀".repeat(2000)));
}

#[test]
fn a_body_that_was_already_cut_is_not_cut_again_and_the_flag_is_kept() {
    let f = Fixture::new();
    f.tell_pm(&"z".repeat(2500));
    let first = f.restart_pm();
    assert_eq!(first["truncated"], true);
    let cut = messages(&first)[0].clone();
    assert!(cut.ends_with(MARKER));
    let second = f.restart_pm();
    assert_eq!(messages(&second)[0], cut, "unchanged");
    assert_eq!(
        second["truncated"], true,
        "the earlier cut is still reported"
    );
    assert_eq!(chars(&messages(&second)[0]), 2000 + chars(MARKER));
}

#[test]
fn a_fresh_body_that_merely_ends_in_the_marker_text_is_still_cut_and_flagged() {
    let f = Fixture::new();
    f.tell_pm(&format!("{}{MARKER}", "q".repeat(2500)));
    let summary = f.restart_pm();
    assert_eq!(summary["truncated"], true);
    assert_eq!(chars(&messages(&summary)[0]), 2011);
}

#[test]
fn a_body_is_cut_where_a_joined_character_ends_not_inside_it() {
    let f = Fixture::new();
    let family = "👨‍👩‍👧";
    f.tell_pm(&format!("{}{family}{}", "x".repeat(1998), "y".repeat(100)));
    let body = messages(&f.restart_pm())[0].clone();
    assert_eq!(
        body,
        format!("{}{MARKER}", "x".repeat(1998)),
        "the family is not split"
    );
}

#[test]
fn a_very_large_task_brief_is_shown_as_a_marked_preview_so_the_summary_always_fits() {
    let mut f = Fixture::new();
    f.close();
    {
        // The Node test dropped the immutability trigger to rewrite the brief; so does this one, on the closed ledger.
        let database = rusqlite::Connection::open(f.database()).unwrap();
        database
            .execute_batch("DROP TRIGGER immutable_project_revisions_update")
            .unwrap();
        database
            .execute(
                "UPDATE project_revisions SET content_json = ?1 WHERE kind = 'task_brief'",
                [serde_json::json!({"text": "y".repeat(16_000)}).to_string()],
            )
            .unwrap();
    }
    f.reopen();
    let summary = f.restart_pm();
    assert_eq!(
        summary["objective"]["truncated"], true,
        "{}",
        summary["objective"]
    );
    let preview = summary["objective"]["rawJsonPreview"].as_str().unwrap();
    assert!(chars(preview) <= 2000, "{}", chars(preview));
    assert!(serde_json::to_string(&summary).unwrap().len() <= 32 * 1024);
}
