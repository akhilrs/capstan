//! Messages from an agent to the user (`areas::user_messages`): `cstan send @user` stores them, the snapshot lists them
//! in `messages` and an open question in `waiting`, and the `message reply` decision answers one.

use crate::event_feed::Rig;
use capstan_kernel::areas::remote_decisions::parse;
use capstan_kernel::areas::snapshot_feed::{
    content_hash, message_hash_input, messages_page, snapshot_item, waiting_page,
};
use capstan_kernel::canonical::sha256;
use capstan_kernel::errors::KernelError;
use serde_json::{json, Value};

fn send(rig: &Rig, who: &str, body: &str, class: &str) -> Result<Value, KernelError> {
    rig.core
        .send_to_user(&rig.ctx(who), &json!({"body": body, "class": class}))
}

fn reply(rig: &Rig, request: &str, expect: &str, nonce: &str, text: &str) -> Result<Value, KernelError> {
    let words: Vec<String> = [request, "--expect", expect, "--nonce", nonce, "reply", text]
        .iter()
        .map(|w| w.to_string())
        .collect();
    let input = parse(&words).unwrap();
    rig.core.decide(&rig.ctx("owner"), &input)
}

fn code(error: KernelError) -> String {
    match error {
        KernelError::MessageTransition { code, .. } => code,
        KernelError::Controller(message) => message.split(':').next().unwrap().to_string(),
        other => panic!("unexpected error {other}"),
    }
}

fn count(rig: &Rig, sql: &str) -> i64 {
    rig.db().query_row(sql, [], |row| row.get(0)).unwrap()
}

fn effects(events: &[Value]) -> Vec<(String, String)> {
    events
        .iter()
        .flat_map(|event| event["body"]["effects"].as_array().cloned().unwrap_or_default())
        .map(|effect| {
            (
                effect["section"].as_str().unwrap().to_string(),
                effect["key"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

#[test]
fn a_question_from_the_pm_is_listed_waits_and_is_answered_by_a_reply() {
    let rig = Rig::new();
    let start = rig.head();
    let sent = send(&rig, "pm-1", "Ship plan-12 today or wait for the review?", "action").unwrap();
    let id = sent["messageId"].as_str().unwrap().to_string();
    assert_eq!(sent["sequence"], json!(1));

    // The messages section lists it, open.
    let page = messages_page(&rig.core, None, 50).unwrap();
    assert_eq!(page.items.len(), 1);
    let item = &page.items[0];
    assert_eq!(item.key, "1");
    assert_eq!(item.sender_agent_id, "pm-1");
    assert_eq!((item.class.as_str(), item.state.as_str()), ("action", "open"));
    assert_eq!(item.body, "Ship plan-12 today or wait for the review?");
    assert_eq!(item.request_id, format!("message:{id}"));
    assert_eq!(page.next, None);

    // It waits, bound to its id and body.
    let waiting = waiting_page(&rig.core, None, 50, Some("message")).unwrap();
    assert_eq!(waiting.items.len(), 1);
    assert_eq!(waiting.items[0].key, "message:1");
    let hash = sha256(&message_hash_input(&id, "Ship plan-12 today or wait for the review?"));
    assert_eq!(waiting.items[0].content_hash, hash);
    let request = format!("message:{id}");
    assert_eq!(content_hash(&rig.core, &request).unwrap(), Some(hash.clone()));

    // The event asks for both items again.
    let sent_effects = effects(&rig.since(start));
    assert!(sent_effects.contains(&("messages".into(), "1".into())), "{sent_effects:?}");
    assert!(sent_effects.contains(&("waiting".into(), "message:1".into())), "{sent_effects:?}");

    // The reply goes to the PM, the question is answered and stops waiting.
    let before = rig.head();
    let answered = reply(&rig, &request, &hash[..12], "nonce-0001", "Wait for the review").unwrap();
    assert_eq!(answered["outcome"], json!("applied"));
    let to_pm: String = rig
        .db()
        .query_row(
            "SELECT body FROM messages WHERE recipient_agent_id = 'pm-1' ORDER BY sequence DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(to_pm.contains("The user answered your message 1"), "{to_pm}");
    assert!(to_pm.contains("Ship plan-12 today"), "{to_pm}");
    assert!(to_pm.ends_with("Wait for the review"), "{to_pm}");
    assert_eq!(
        count(&rig, "SELECT count(*) FROM user_messages WHERE state = 'answered' AND reply_message_id IS NOT NULL"),
        1
    );
    assert!(waiting_page(&rig.core, None, 50, Some("message")).unwrap().items.is_empty());
    assert_eq!(content_hash(&rig.core, &request).unwrap(), None);
    let listed = snapshot_item(&rig.core, "messages", "1").unwrap().unwrap();
    assert_eq!(listed["state"], json!("answered"));
    let reply_effects = effects(&rig.since(before));
    assert!(reply_effects.contains(&("waiting".into(), "message:1".into())), "{reply_effects:?}");
    assert!(reply_effects.contains(&("messages".into(), "1".into())), "{reply_effects:?}");

    // A second reply finds nothing waiting.
    assert_eq!(
        code(reply(&rig, &request, &hash[..12], "nonce-0002", "again").unwrap_err()),
        "not_waiting"
    );
}

#[test]
fn an_info_message_is_listed_and_does_not_wait() {
    let rig = Rig::new();
    send(&rig, "pm-1", "plan-12 is merged.", "info").unwrap();
    send(&rig, "dev-1", "Started package 3.", "info").unwrap();
    let page = messages_page(&rig.core, None, 1).unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].state, "info");
    assert_eq!(page.next.as_deref(), Some("1"));
    let rest = messages_page(&rig.core, Some("1"), 10).unwrap();
    assert_eq!(rest.items.len(), 1);
    assert_eq!(rest.items[0].sender_agent_id, "dev-1");
    assert_eq!(rest.next, None);
    assert!(waiting_page(&rig.core, None, 50, Some("message")).unwrap().items.is_empty());
}

#[test]
fn only_an_agent_sends_and_sixty_an_hour_is_the_limit() {
    let rig = Rig::new();
    assert_eq!(code(send(&rig, "owner", "hello", "info").unwrap_err()), "not_an_agent");
    assert!(send(&rig, "pm-1", "x", "loud").is_err());
    for n in 0..60 {
        send(&rig, "pm-1", &format!("note {n}"), "info").unwrap();
    }
    assert_eq!(code(send(&rig, "pm-1", "one more", "info").unwrap_err()), "limit_reached");
    assert_eq!(count(&rig, "SELECT count(*) FROM user_messages"), 60);
}

#[test]
fn a_question_whose_agent_is_gone_cannot_be_answered() {
    let rig = Rig::new();
    let sent = send(&rig, "dev-1", "Which port?", "action").unwrap();
    let id = sent["messageId"].as_str().unwrap();
    rig.db()
        .execute("UPDATE agents SET state = 'ended' WHERE agent_id = 'dev-1'", [])
        .unwrap();
    let request = format!("message:{id}");
    let hash = content_hash(&rig.core, &request).unwrap().unwrap();
    assert_eq!(
        code(reply(&rig, &request, &hash[..12], "nonce-0001", "8080").unwrap_err()),
        "not_waiting"
    );
    assert_eq!(count(&rig, "SELECT count(*) FROM user_messages WHERE state = 'open'"), 1);
}

#[test]
fn the_asking_agent_closes_a_question_answered_in_the_terminal() {
    let rig = Rig::new();
    let sent = send(&rig, "pm-1", "Ship today? 1. Yes 2. No", "action").unwrap();
    let id = sent["messageId"].as_str().unwrap().to_string();
    let close = |who: &str, n: i64| rig.core.close_user_question(&rig.ctx(who), &json!({"sequence": n}));
    assert_eq!(code(close("dev-1", 1).unwrap_err()), "not_yours");
    assert_eq!(code(close("pm-1", 9).unwrap_err()), "not_open");
    let start = rig.head();
    close("pm-1", 1).unwrap();
    assert!(waiting_page(&rig.core, None, 50, Some("message")).unwrap().items.is_empty());
    assert_eq!(content_hash(&rig.core, &format!("message:{id}")).unwrap(), None);
    let listed = snapshot_item(&rig.core, "messages", "1").unwrap().unwrap();
    assert_eq!(listed["state"], json!("answered"));
    let closed_effects = effects(&rig.since(start));
    assert!(closed_effects.contains(&("messages".into(), "1".into())), "{closed_effects:?}");
    assert!(closed_effects.contains(&("waiting".into(), "message:1".into())), "{closed_effects:?}");
    assert_eq!(code(close("pm-1", 1).unwrap_err()), "not_open");
}
