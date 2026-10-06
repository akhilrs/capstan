//! The view layer against the frames, overlays and format tables the Node implementation exports.
mod common;

use common::{assert_json_eq_in, case_names, load_fixture};
use cstan_dash::model::*;
use cstan_dash::view::format::{
    age, age_detail, commit_short, duration_text, fit, sparkline, task_summary, truncate,
};
use cstan_dash::view::theme::{gradient_color, palette};
use cstan_dash::view::{
    build_frame, cell_width, confirm_overlay, footer_hints, help_overlay, make_theme,
    observe_overlay, ColorRole, PanelId, PeekView, Size, Theme, ViewState,
};
use serde_json::{json, Value};

/// What `OBSERVE_PEEK` in `test/dash-golden-cases.ts` holds.
fn peek() -> PeekView {
    PeekView {
        agent_id: "developer-2".to_string(),
        agent_status: "blocked".to_string(),
        text: "developer-2 pane p4\n\nReading docs/spike-herdr-agents.md\nEdit(src/dash/theme.ts)\n  Updated src/dash/theme.ts with 12 additions\n\nThinking... (3m 41s, esc to interrupt)\n\n  waiting for tool permission:\n  Bash(npm run check)\n  1. Yes   2. No\n".to_string(),
    }
}

fn text_of(frame: &Value) -> Vec<String> {
    frame["lines"]
        .as_array()
        .expect("lines")
        .iter()
        .map(|line| {
            line.as_array()
                .expect("line")
                .iter()
                .map(|s| s["text"].as_str().expect("text"))
                .collect()
        })
        .collect()
}

#[test]
fn every_frame_and_overlay_equals_its_fixture() {
    let names = case_names();
    assert!(names.len() >= 13, "cases: {names:?}");
    for name in names {
        let fixture = load_fixture(&name);
        let model: DashModel = serde_json::from_value(fixture["model"].clone()).expect("model");
        for (i, entry) in fixture["views"]
            .as_array()
            .expect("views")
            .iter()
            .enumerate()
        {
            let at = format!("{name}.views[{i}]");
            let view: ViewState = serde_json::from_value(entry["view"].clone()).expect("view");
            let options = serde_json::from_value(entry["theme"].clone()).expect("theme");
            let theme: Theme = make_theme(options);
            let size = view.size;

            let frame = build_frame(&model, &view, &theme);
            let got = serde_json::to_value(&frame).unwrap();
            assert_json_eq_in(&format!("{at}.frame"), &got, &entry["frame"]);

            let want = text_of(&entry["frame"]);
            assert_eq!(
                want.len(),
                size.rows as usize,
                "{at}: the fixture fills the rows"
            );
            for (r, line) in text_of(&got).iter().enumerate() {
                assert_eq!(
                    cell_width(line),
                    size.columns as usize,
                    "{at}: row {r} fills the columns"
                );
            }

            let overlays = &entry["overlays"];
            let got = serde_json::to_value(help_overlay(size, &theme)).unwrap();
            assert_json_eq_in(&format!("{at}.help"), &got, &overlays["help"]);
            let got = serde_json::to_value(observe_overlay(&peek(), size, &theme)).unwrap();
            assert_json_eq_in(&format!("{at}.observe"), &got, &overlays["observe"]);
            for (j, confirm) in overlays["confirm"]
                .as_array()
                .expect("confirm")
                .iter()
                .enumerate()
            {
                let action: DashAction =
                    serde_json::from_value(confirm["action"].clone()).expect("action");
                let got = serde_json::to_value(confirm_overlay(&action, size, &theme)).unwrap();
                assert_json_eq_in(&format!("{at}.confirm[{j}]"), &got, &confirm["overlay"]);
            }
        }
    }
}

#[test]
fn the_fixtures_cover_the_tiny_narrow_and_wide_modes() {
    let mut sizes = std::collections::BTreeSet::new();
    for name in case_names() {
        for entry in load_fixture(&name)["views"].as_array().unwrap() {
            let size: Size = serde_json::from_value(entry["view"]["size"].clone()).unwrap();
            sizes.insert((size.columns, size.rows));
        }
    }
    assert!(
        sizes.iter().any(|&(c, r)| c >= 100 && r >= 16),
        "wide: {sizes:?}"
    );
    assert!(
        sizes
            .iter()
            .any(|&(c, r)| (60..100).contains(&c) && r >= 16),
        "narrow: {sizes:?}"
    );
}

#[test]
fn the_format_tables_match() {
    let f = load_fixture("format");
    let at = |part: &str, i: usize| format!("{part}[{i}]");
    for (i, e) in f["cellWidth"].as_array().unwrap().iter().enumerate() {
        assert_eq!(
            json!(cell_width(e["text"].as_str().unwrap())),
            e["out"],
            "{}",
            at("cellWidth", i)
        );
    }
    for (i, e) in f["truncate"].as_array().unwrap().iter().enumerate() {
        let got = truncate(
            e["text"].as_str().unwrap(),
            e["width"].as_u64().unwrap() as usize,
        );
        assert_eq!(json!(got), e["out"], "{}", at("truncate", i));
    }
    for (i, e) in f["fit"].as_array().unwrap().iter().enumerate() {
        let got = fit(
            e["text"].as_str().unwrap(),
            e["width"].as_u64().unwrap() as usize,
        );
        assert_eq!(json!(got), e["out"], "{}", at("fit", i));
    }
    for (i, e) in f["age"].as_array().unwrap().iter().enumerate() {
        let got = age(e["iso"].as_str(), e["nowMs"].as_i64().unwrap());
        assert_eq!(json!(got), e["out"], "{}", at("age", i));
    }
    for (i, e) in f["ageDetail"].as_array().unwrap().iter().enumerate() {
        let got = age_detail(e["iso"].as_str(), e["nowMs"].as_i64().unwrap());
        assert_eq!(json!(got), e["out"], "{}", at("ageDetail", i));
    }
    for (i, e) in f["durationText"].as_array().unwrap().iter().enumerate() {
        let got = duration_text(e["seconds"].as_i64().unwrap());
        assert_eq!(json!(got), e["out"], "{}", at("durationText", i));
    }
    for (i, e) in f["commitShort"].as_array().unwrap().iter().enumerate() {
        assert_eq!(
            json!(commit_short(e["sha"].as_str())),
            e["out"],
            "{}",
            at("commitShort", i)
        );
    }
    for (i, e) in f["sparkline"].as_array().unwrap().iter().enumerate() {
        let values: Vec<f64> = e["values"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect();
        let got = sparkline(&values, e["width"].as_u64().unwrap() as usize);
        assert_eq!(json!(got), e["out"], "{}", at("sparkline", i));
    }
    for (i, e) in f["taskSummary"].as_array().unwrap().iter().enumerate() {
        let tasks: Option<Vec<TaskEntry>> = serde_json::from_value(e["tasks"].clone()).unwrap();
        let got = task_summary(tasks.as_deref(), e["width"].as_u64().unwrap() as usize);
        assert_eq!(json!(got), e["out"], "{}", at("taskSummary", i));
    }
}

#[test]
fn the_palette_gradient_and_hints_match() {
    let f = load_fixture("format");
    for (role, hex) in f["theme"]["palette"].as_object().unwrap() {
        let role: ColorRole = serde_json::from_value(json!(role)).expect("role");
        assert_eq!(json!(palette(role)), *hex);
        let theme = make_theme(Default::default());
        assert_eq!(theme.color(role).as_deref(), hex.as_str());
    }
    let plain = make_theme(cstan_dash::view::ThemeOptions {
        no_color: true,
        ..Default::default()
    });
    assert_eq!(plain.color(ColorRole::Ok), None);
    assert_eq!(plain.gradient(0.5), None);
    for e in f["theme"]["gradient"].as_array().unwrap() {
        let got = gradient_color(e["fraction"].as_f64().unwrap());
        assert_eq!(json!(got), e["out"], "gradient {}", e["fraction"]);
    }
    for e in f["hints"].as_array().unwrap() {
        let focus: PanelId = serde_json::from_value(e["focus"].clone()).unwrap();
        let got = footer_hints(focus, e["ascii"].as_bool().unwrap());
        assert_eq!(serde_json::to_value(got).unwrap(), e["out"], "hints {e}");
    }
}
