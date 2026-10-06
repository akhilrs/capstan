//! `build_dash_model` against the models the Node implementation exports for every fixture.
mod common;

use common::{assert_json_eq_in, case_names, load_fixture};
use cstan_dash::model::build_dash_model;

#[test]
fn every_fixture_builds_the_model_node_builds() {
    let names = case_names();
    assert!(!names.is_empty(), "no fixtures found");
    for name in names {
        let fixture = load_fixture(&name);
        let now_ms = fixture["nowMs"].as_i64().expect("nowMs");
        let worker_limit = fixture["workerLimit"].as_i64();
        let model = build_dash_model(&fixture["status"], now_ms, worker_limit);
        let actual = serde_json::to_value(&model).expect("model serialises");
        assert_json_eq_in(&format!("{name}.model"), &actual, &fixture["model"]);
    }
}
