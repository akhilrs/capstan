//! The write-once guard (`areas::write_once`): the scan of Rust source for writes to guarded columns, the check of every
//! `update_message` call site, and the snapshot of a row's guarded columns.

use crate::fixture::{context, Fixture, OWNER};
use capstan_kernel::areas::write_once::{
    update_message_calls, write_once_snapshot, write_once_violations,
    write_once_violations_in_tree, write_once_violations_with, WRITE_ONCE, WRITE_ONCE_ALLOW,
};
use serde_json::json;
use std::path::Path;

fn src() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src"))
}

fn flagged(source: &str) -> bool {
    !write_once_violations(source).is_empty()
}

#[test]
fn a_write_to_a_guarded_column_is_reported() {
    for source in [
        r#"fn f() { run("UPDATE messages SET body = ? WHERE message_id = ?"); }"#,
        r#"fn f() { run("UPDATE agents SET role_name = ? WHERE agent_id = ?"); }"#,
        "fn f() { run(\"UPDATE prompt_relays SET state = ?,\n   prompt_text = ?\n WHERE relay_id = ?\"); }",
        "fn f() { run(\"UPDATE prompt_relays SET state = ?, \\\n        prompt_text = ? WHERE relay_id = ?\"); }",
        r#"fn f() { run("INSERT OR REPLACE INTO messages(body) VALUES (?)"); }"#,
        r#"fn f() { run("REPLACE INTO prompt_relays(relay_id) VALUES (?)"); }"#,
        r#"fn f() { run("INSERT INTO operator_proposals(proposal_id, command) VALUES (?, ?) ON CONFLICT(proposal_id) DO UPDATE SET command = ?"); }"#,
        r##"fn f() { run(r#"update agent_reports set summary = 'x'"#); }"##,
        r#"fn f() { update_message(kernel, &row, "x", &[("body", Sql::Null)], &now)?; }"#,
    ] {
        assert!(flagged(source), "not reported: {source}");
    }
}

#[test]
fn the_real_shapes_that_write_only_other_columns_are_not_reported() {
    for source in [
        r#"fn f() { run("UPDATE agents SET last_activity_at = ? WHERE project_id = ? AND agent_id = ?"); }"#,
        r#"fn f() { run("UPDATE agents SET actor_id = ?, generation = ?, last_activity_at = ? WHERE agent_id = ?"); }"#,
        r#"fn f() { run("UPDATE messages SET notified_at = ?, state_version = state_version + 1 WHERE message_id = ?"); }"#,
        "fn f() { run(\"UPDATE prompt_relays SET state = 'answered',\n  answer_text = ?,\n  answered_at = ?\n WHERE relay_id = ?\"); }",
        r#"fn f() { run("UPDATE agents SET state = (SELECT kind FROM roles WHERE a = b) WHERE agent_id = ?"); }"#,
        r#"fn f() { run("UPDATE other_table SET body = ?"); }"#,
        r#"fn f() { run("INSERT INTO messages(body) VALUES (?)"); }"#,
        r#"fn f() { update_message(kernel, &row, "x", &[("sent_at", Sql::Null)], &now)?; }"#,
        r#"fn f() { update_message(kernel, &row, "x", &[], &now)?; }"#,
        r#"fn f() { run("UPDATE messages SET state = ?, updated_at = ?{columns} WHERE message_id = ?"); }"#,
    ] {
        assert_eq!(
            write_once_violations(source),
            Vec::<String>::new(),
            "reported: {source}"
        );
    }
}

#[test]
fn a_comment_and_a_test_module_are_ignored() {
    let source = r#"
// UPDATE messages SET body = ?
/* UPDATE messages SET body = ? /* nested */ UPDATE agents SET kind = ? */
/// run("UPDATE messages SET body = ?")
fn f() { let c = '"'; let lifetime: &'static str = "ok"; let _ = (c, lifetime); }
#[cfg(test)]
mod tests {
    fn g() { run("UPDATE messages SET body = ?"); update_message(k, &r, "x", &[("body", v)], &n); }
}
"#;
    assert_eq!(write_once_violations(source), Vec::<String>::new());
    // The same text in live code is reported.
    assert!(flagged(
        "fn f() { run(\"UPDATE messages SET body = ?\"); }\n#[cfg(test)]\nmod tests {}"
    ));
}

#[test]
fn an_allow_entry_silences_exactly_its_match() {
    let call = r#"fn f() { update_message(kernel, &row, "x", &extra, &now)?; }"#;
    assert_eq!(write_once_violations_with(call, &[]).len(), 1);
    let allow = [(
        r#"update_message(kernel, &row, "x", &extra, &now)"#,
        "a reviewed reason",
    )];
    assert!(write_once_violations_with(call, &allow).is_empty());
    let other = r#"fn f() { update_message(kernel, &row, "y", &extra, &now)?; }"#;
    assert_eq!(write_once_violations_with(other, &allow).len(), 1);
    for (text, reason) in WRITE_ONCE_ALLOW {
        assert!(!text.is_empty() && !reason.is_empty());
    }
}

#[test]
fn nothing_in_the_kernel_source_writes_a_guarded_column() {
    let found = write_once_violations_in_tree(src()).expect("the source tree reads");
    assert!(
        found.is_empty(),
        "write-once violations:\n{}",
        found.join("\n")
    );
}

#[test]
fn every_update_message_call_site_passes_a_slice_literal_of_name_value_tuples() {
    let mut sites = 0;
    let mut pending = vec![src().to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let source = std::fs::read_to_string(&path).unwrap();
                for call in update_message_calls(&source) {
                    sites += 1;
                    let allowed = WRITE_ONCE_ALLOW.iter().any(|(text, _)| *text == call.text);
                    if let Err(reason) = &call.columns {
                        assert!(allowed, "{}: {reason}", path.display());
                    }
                }
            }
        }
    }
    assert!(sites >= 10, "only {sites} call sites found");
}

#[test]
fn a_call_passing_a_variable_is_unscannable() {
    let calls = update_message_calls(
        r#"fn f(extra: &[(&str, Sql)]) { update_message(kernel, &row, "x", extra, &now)?; update_message(k, &r, "x", &[("a", Sql::Null), ("b", v)], &n)?; }
           pub fn update_message(kernel: &Kernel, row: &MessageRow, to: &str, extra: &[(&str, Sql)], now: &str) {}"#,
    );
    assert_eq!(calls.len(), 2);
    assert!(calls[0].columns.is_err());
    assert_eq!(calls[1].columns.as_ref().unwrap(), &["a", "b"]);
    assert!(
        write_once_violations("fn f() { update_message(k, &r, \"x\", cols, &n); }")
            .iter()
            .any(|v| v.starts_with("unscannable"))
    );
}

#[test]
fn the_snapshot_returns_a_rows_guarded_columns_and_refuses_other_tables() {
    let f = Fixture::new();
    let id = f
        .core()
        .enqueue_message(
            &context(f.core(), OWNER),
            &json!({"recipientAgentId": "dev-1", "body": "hello"}),
        )
        .unwrap()["messageId"]
        .as_str()
        .unwrap()
        .to_string();
    let snapshot = write_once_snapshot(f.core(), "messages", &id).unwrap();
    let names: Vec<&str> = snapshot.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        ["body", "recipient_agent_id", "action_needed", "class"]
    );
    assert_eq!(snapshot[0].1, json!("hello"));
    assert_eq!(snapshot[1].1, json!("dev-1"));
    // Every guarded table reads (the columns exist) and a missing id is empty.
    for (table, _) in WRITE_ONCE {
        assert!(write_once_snapshot(f.core(), table, "no-such-id")
            .unwrap()
            .is_empty());
    }
    let agent = write_once_snapshot(f.core(), "agents", "dev-1").unwrap();
    assert_eq!(agent.len(), 3);
    assert!(write_once_snapshot(f.core(), "message_events", &id).is_err());
    assert!(write_once_snapshot(f.core(), "messages; DROP TABLE messages", &id).is_err());
}
