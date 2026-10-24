//! The launcher fills the architect's design note from `[design]` when it builds the architect's prompt: nothing for the
//! default configuration, the pinned note for a changed design record, and only in the architect's own prompt.
mod common;

use capstan_launcher::api::LauncherService;
use common::engine::Scenario;
use serde_json::json;

const NOTE: &str = "In this project the design record is docs/web/DESIGN.md, docs/app/DESIGN.md (mode maintain). The exception above applies to each of these paths: exactly one design package of the plan owns each path it changes.";

fn toml(design: &str) -> String {
    format!(
        r#"schema_version = 1
[nexora]
track = "never"
[hosts.claude]
kind = "claude"
[architect]
enabled = true
[roles.pm]
kind = "PM"
host = "claude"
[roles.architect]
kind = "Developer"
host = "claude"
[roles.developer]
kind = "Developer"
host = "claude"
{design}"#
    )
}

/// The prompts of the architect and of a developer, in that order.
fn prompts(name: &str, design: &str) -> (String, String) {
    let sc = Scenario::new(name, json!({"toml": toml(design), "architect": {}}));
    sc.launcher.launch_pm().expect("the PM launches");
    for role in ["architect", "developer"] {
        sc.launcher
            .spawn(role, &Default::default())
            .unwrap_or_else(|e| panic!("spawn {role}: {} {}", e.code, e.message));
    }
    let all = sc.prompts();
    let architect = all
        .iter()
        .find(|text| text.contains("You are the architect"))
        .expect("the architect's prompt")
        .clone();
    (architect, all.last().unwrap().clone())
}

#[test]
fn the_default_configuration_adds_no_note() {
    let (architect, _) = prompts("design-note-default", "");
    assert!(!architect.contains("In this project the design record is"));
}

#[test]
fn a_changed_design_record_puts_the_note_in_the_architect_prompt_once() {
    let design = "[design]\ndesign_record = [\"docs/web/DESIGN.md\", \"docs/app/DESIGN.md\"]\n";
    let (architect, developer) = prompts("design-note-two-paths", design);
    assert_eq!(architect.matches(NOTE).count(), 1, "{architect}");
    assert!(!developer.contains("In this project the design record is"));
}
