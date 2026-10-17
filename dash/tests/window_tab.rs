//! The auto-approve window tab in the header (plan-44 d1, direction A): text, place and drop order.
mod common;
use common::{case_names, load_fixture};
use cstan_dash::model::*;
use cstan_dash::view::theme::make_theme;
use cstan_dash::view::{build_frame, Theme, ViewState};
use serde_json::{json, Value};

/// The top row of the header at `columns`, for a model whose header is patched with `header`.
fn top_row(columns: u64, header: Value) -> String {
    let fixture = load_fixture(&case_names()[0]);
    let mut model = fixture["model"].clone();
    for (key, value) in header.as_object().unwrap() {
        model["header"][key] = value.clone();
    }
    let model: DashModel = serde_json::from_value(model).expect("model");
    let entry = &fixture["views"][0];
    let mut view = entry["view"].clone();
    view["size"]["columns"] = json!(columns);
    view["size"]["rows"] = json!(40);
    let view: ViewState = serde_json::from_value(view).expect("view");
    let theme: Theme = make_theme(serde_json::from_value(entry["theme"].clone()).expect("theme"));
    let frame = serde_json::to_value(build_frame(&model, &view, &theme)).unwrap();
    frame["lines"][0]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["text"].as_str().unwrap())
        .collect()
}

fn base() -> Value {
    json!({"projectId": "pe7275bc45ede472f8acaa75ce193ee32", "runState": "active", "runPause": null})
}

fn with(extra: Value) -> Value {
    let mut header = base();
    for (k, v) in extra.as_object().unwrap() {
        header[k] = v.clone();
    }
    header
}

#[test]
fn an_open_window_shows_the_minutes_and_costs_the_project_tab() {
    let row = top_row(
        80,
        with(json!({"window": {"minutes": 42, "operatorActive": true}})),
    );
    assert!(row.contains("AUTO-APPROVE 42m "), "{row}");
    assert!(!row.contains("workers"), "{row}");
    assert!(!row.contains("pe7275"), "{row}");
}

#[test]
fn an_ended_operator_part_says_workers() {
    let row = top_row(
        80,
        with(json!({"window": {"minutes": 42, "operatorActive": false}})),
    );
    assert!(row.contains("AUTO-APPROVE 42m workers"), "{row}");
}

#[test]
fn full_auto_window_and_grants_keep_their_order() {
    let row = top_row(
        80,
        with(
            json!({"fullAutoMinutes": 12, "grants": 2, "window": {"minutes": 42, "operatorActive": true}}),
        ),
    );
    let at = |needle: &str| {
        row.find(needle)
            .unwrap_or_else(|| panic!("{needle} in {row}"))
    };
    assert!(at("run active") < at("FULL AUTO 12m"));
    assert!(at("FULL AUTO 12m") < at("AUTO-APPROVE 42m"));
    assert!(at("AUTO-APPROVE 42m") < at("grants 2"));
}

#[test]
fn full_auto_and_the_window_are_the_last_tabs_dropped() {
    let row = top_row(
        60,
        with(
            json!({"fullAutoMinutes": 12, "grants": 2, "window": {"minutes": 42, "operatorActive": true}}),
        ),
    );
    assert!(row.contains("FULL AUTO 12m"), "{row}");
    assert!(row.contains("AUTO-APPROVE 42m"), "{row}");
    assert!(!row.contains("grants"), "{row}");
    assert!(!row.contains("run active"), "{row}");
}

#[test]
fn no_window_leaves_the_header_as_it_was() {
    let row = top_row(80, base());
    assert!(!row.contains("AUTO-APPROVE"), "{row}");
    assert!(row.contains("pe7275bc45ede472f8acaa75ce193ee32"), "{row}");
}

#[test]
fn sixty_one_seconds_is_a_two_minute_tab() {
    let model = cstan_dash::model::build_dash_model(
        &json!({"operator": {"window": {"windowId": "w", "operatorActive": true, "remainingSeconds": 61}}}),
        0,
        None,
    );
    assert_eq!(
        model.header.window,
        Some(WindowHeader {
            minutes: 2,
            operator_active: true
        })
    );
    let none = cstan_dash::model::build_dash_model(&json!({"operator": {}}), 0, None);
    assert_eq!(none.header.window, None);
}

#[test]
fn every_window_string_says_auto_approve_and_never_full_auto() {
    let table: Value = serde_json::from_str(include_str!(
        "../../docs/design/plan-44-d1-design-strings.json"
    ))
    .unwrap();
    for row in table.as_array().unwrap() {
        let template = row["template"].as_str().unwrap();
        assert!(template.starts_with("AUTO-APPROVE"), "{template}");
        assert!(!template.to_lowercase().contains("full auto"), "{template}");
    }
}
