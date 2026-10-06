//! The shared types and the model helpers against the fixtures the Node implementation exports.
mod common;

use common::{assert_json_eq, assert_json_eq_in, case_names, load_fixture};
use cstan_dash::model::text::clean;
use cstan_dash::model::*;
use cstan_dash::view::{Frame, Overlay, Theme, ViewState};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};

/// Deserialises `value` into `T` and checks that serialising it again gives the same JSON.
fn roundtrip<T: Serialize + DeserializeOwned>(label: &str, value: &Value) -> T {
    let typed: T = serde_json::from_value(value.clone())
        .unwrap_or_else(|e| panic!("{label} does not deserialise: {e}"));
    let again = serde_json::to_value(&typed).expect("serialises");
    assert_json_eq_in(label, &again, value);
    typed
}

fn ids<T: HasId>(rows: Vec<&T>) -> Vec<String> {
    rows.into_iter().map(|r| r.id().to_string()).collect()
}

trait HasId {
    fn id(&self) -> &str;
}
impl HasId for AgentRow {
    fn id(&self) -> &str {
        &self.id
    }
}
impl HasId for MessageRow {
    fn id(&self) -> &str {
        &self.id
    }
}
impl HasId for PipelineItem {
    fn id(&self) -> &str {
        &self.id
    }
}

/// The poll the exporter simulates: the first agent changed and the first message resolved.
fn next_poll(model: &DashModel) -> DashModel {
    let mut next = model.clone();
    if let Some(agent) = next.agents.first_mut() {
        agent.fingerprint.push('x');
    }
    if !next.queue.messages.is_empty() {
        next.queue.messages.remove(0);
    }
    next
}

#[test]
fn every_case_deserialises_and_serialises_back() {
    let names = case_names();
    assert!(names.len() >= 13, "cases: {names:?}");
    for name in names {
        let fixture = load_fixture(&name);
        let _: DashModel = roundtrip(&format!("{name}.model"), &fixture["model"]);
        let views = fixture["views"].as_array().expect("views");
        assert!(!views.is_empty(), "{name} has no views");
        for (i, entry) in views.iter().enumerate() {
            let at = format!("{name}.views[{i}]");
            let _: ViewState = roundtrip(&format!("{at}.view"), &entry["view"]);
            let _: Theme = roundtrip(&format!("{at}.theme"), &entry["theme"]);
            let _: Frame = roundtrip(&format!("{at}.frame"), &entry["frame"]);
            let overlays = &entry["overlays"];
            let _: Overlay = roundtrip(&format!("{at}.help"), &overlays["help"]);
            let _: Overlay = roundtrip(&format!("{at}.observe"), &overlays["observe"]);
            for (j, confirm) in overlays["confirm"]
                .as_array()
                .expect("confirm")
                .iter()
                .enumerate()
            {
                let _: DashAction =
                    roundtrip(&format!("{at}.confirm[{j}].action"), &confirm["action"]);
                let _: Overlay =
                    roundtrip(&format!("{at}.confirm[{j}].overlay"), &confirm["overlay"]);
            }
        }
    }
}

#[test]
fn the_helpers_match_the_exported_helpers() {
    for name in case_names() {
        let fixture = load_fixture(&name);
        let model: DashModel = serde_json::from_value(fixture["model"].clone()).expect("model");
        let want = &fixture["helpers"];
        let now_ms = fixture["nowMs"].as_i64().expect("nowMs");
        let at = |part: &str| format!("{name}.helpers.{part}");

        let got = json!({
            "recent": ids(visible_agents(&model, false)),
            "all": ids(visible_agents(&model, true)),
        });
        assert_json_eq_in(&at("visibleAgents"), &got, &want["visibleAgents"]);

        let got = json!({
            "all": ids(queue_rows(&model, false)),
            "problems": ids(queue_rows(&model, true)),
        });
        assert_json_eq_in(&at("queueRows"), &got, &want["queueRows"]);

        assert_eq!(
            json!(queue_collapsed(&model)),
            want["queueCollapsed"],
            "{name}"
        );
        assert_json_eq_in(
            &at("pipelineItems"),
            &json!(ids(pipeline_items(&model))),
            &want["pipelineItems"],
        );
        assert_json_eq_in(
            &at("historySample"),
            &serde_json::to_value(history_sample(&model, now_ms)).unwrap(),
            &want["historySample"],
        );
        assert_json_eq_in(
            &at("historySampleLater"),
            &serde_json::to_value(history_sample(&model, now_ms + 90_500)).unwrap(),
            &want["historySampleLater"],
        );

        let changed = next_poll(&model);
        let first = track_changes(&ChangeState::default(), &model);
        let second = track_changes(&first, &changed);
        let third = track_changes(&second, &changed);
        let fourth = track_changes(&third, &changed);
        let got = serde_json::to_value([first, second, third, fourth]).unwrap();
        assert_json_eq_in(&at("trackChanges"), &got, &want["trackChanges"]);
    }
}

#[test]
fn the_first_poll_highlights_nothing_and_a_change_fades_after_the_highlight_polls() {
    let fixture = load_fixture("showcase");
    let model: DashModel = serde_json::from_value(fixture["model"].clone()).unwrap();
    let first = track_changes(&ChangeState::default(), &model);
    assert!(first.highlight.is_empty());
    let changed = next_poll(&model);
    let second = track_changes(&first, &changed);
    assert_eq!(second.highlight.len(), 1);
    assert_eq!(second.highlight[0].1, HIGHLIGHT_POLLS);
    let third = track_changes(&second, &changed);
    assert_eq!(third.highlight, vec![(second.highlight[0].0.clone(), 1)]);
    assert!(track_changes(&third, &changed).highlight.is_empty());
}

#[test]
fn the_actions_match_the_exported_actions() {
    let fixture = load_fixture("actions");
    for key in ACTION_KEYS {
        assert_eq!(json!(key.0.key()), fixture["keys"][key.0.as_str()]);
        assert_eq!(key.0.key(), key.1);
    }
    for (i, entry) in fixture["available"].as_array().unwrap().iter().enumerate() {
        let message: MessageRow = roundtrip(&format!("available[{i}].message"), &entry["message"]);
        let got = serde_json::to_value(available_decisions(&message)).unwrap();
        assert_json_eq_in(&format!("available[{i}]"), &got, &entry["out"]);
    }
    for (i, entry) in fixture["observe"].as_array().unwrap().iter().enumerate() {
        let agent: AgentRow = roundtrip(&format!("observe[{i}].agent"), &entry["agent"]);
        let got = serde_json::to_value(observe_action(&agent)).unwrap();
        assert_json_eq_in(&format!("observe[{i}]"), &got, &entry["out"]);
    }
    for (i, entry) in fixture["confirm"].as_array().unwrap().iter().enumerate() {
        let action: DashAction = roundtrip(&format!("confirm[{i}].action"), &entry["action"]);
        assert_eq!(json!(confirm_text(&action)), entry["out"], "confirm[{i}]");
    }
    for (i, entry) in fixture["wire"].as_array().unwrap().iter().enumerate() {
        let action: DashAction = serde_json::from_value(entry["action"].clone()).unwrap();
        let (command, args) = to_wire_call(&action);
        let got = json!({ "command": command, "args": args });
        assert_json_eq_in(&format!("wire[{i}]"), &got, &entry["out"]);
    }
}

#[test]
fn clean_and_the_worker_rule_match_the_exported_format_fixture() {
    let fixture = load_fixture("format");
    for (i, entry) in fixture["clean"].as_array().unwrap().iter().enumerate() {
        let text = entry["text"].as_str().unwrap();
        assert_eq!(json!(clean(text)), entry["out"], "clean[{i}]");
    }
    for entry in fixture["isWorkerKind"].as_array().unwrap() {
        let kind = entry["kind"].as_str().unwrap();
        assert_eq!(json!(is_worker_kind(kind)), entry["out"], "{kind}");
    }
    let constants = &fixture["constants"];
    assert_eq!(json!(WORKING_WINDOW_MS), constants["workingWindowMs"]);
    assert_eq!(json!(PIPELINE_ITEMS), constants["pipelineItems"]);
    assert_eq!(json!(HIGHLIGHT_POLLS), constants["highlightPolls"]);
    assert_eq!(json!(RECENT_ENDED_SHOWN), constants["recentEndedShown"]);
    assert_eq!(json!(TASK_NONE), constants["taskNone"]);
}

#[test]
fn json_comparison_ignores_number_formatting_but_not_structure() {
    assert_json_eq(&json!({"a": [1, 2.0, -0.0]}), &json!({"a": [1.0, 2, 0]}));
    let result =
        std::panic::catch_unwind(|| assert_json_eq(&json!({"a": 1}), &json!({"a": 1, "b": null})));
    assert!(result.is_err(), "a missing key is a difference");
    let result = std::panic::catch_unwind(|| assert_json_eq(&json!([1, 2]), &json!([2, 1])));
    assert!(result.is_err(), "array order matters");
}

#[test]
fn iso_timestamps_parse_like_date_parse() {
    use cstan_dash::model::time::parse_iso_ms;
    assert_eq!(
        parse_iso_ms("2026-10-02T12:00:00.000Z"),
        Some(1_790_942_400_000)
    );
    assert_eq!(
        parse_iso_ms("2026-10-02T12:00:00Z"),
        Some(1_790_942_400_000)
    );
    assert_eq!(
        parse_iso_ms("2026-10-02T14:00:00+02:00"),
        Some(1_790_942_400_000)
    );
    assert_eq!(
        parse_iso_ms("2026-10-02T12:00:00.5Z"),
        Some(1_790_942_400_500)
    );
    assert_eq!(parse_iso_ms("2026-10-02"), Some(1_790_899_200_000));
    assert_eq!(parse_iso_ms("not a date"), None);
    assert_eq!(parse_iso_ms(""), None);
}
