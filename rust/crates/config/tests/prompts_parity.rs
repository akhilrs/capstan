//! Every prompt variant of test/config-parity-export.ts (the prompts the Node tests build, the six golden prompts and a
//! matrix of roles and flags) is built here and must equal Node's text byte for byte: same length and SHA-256, and for the
//! golden ones the very bytes of rust/testdata/fixtures/prompts-off.
mod common;

use std::collections::HashMap;

use capstan_config::prompts::*;
use capstan_wire::js::Value;
use common::*;

fn strings(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| match item {
                Value::String(s) => s.to_utf8().expect("text"),
                other => panic!("{other:?}"),
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn opt(value: &Value, key: &str) -> Option<String> {
    text(value, key)
}

fn leak(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

fn summary(value: &Value) -> RestartSummary {
    let list = |key: &str| -> Vec<Value> {
        match value.get(key) {
            Some(Value::Array(items)) => items.clone(),
            _ => Vec::new(),
        }
    };
    RestartSummary {
        objective: value.get("objective").cloned().unwrap_or(Value::Null),
        open_work: list("openWork")
            .iter()
            .map(|item| OpenWork {
                work_item_id: text(item, "workItemId").unwrap(),
                title: text(item, "title").unwrap(),
                role: text(item, "role").unwrap(),
                state: text(item, "state").unwrap(),
                owner: opt(item, "owner"),
                blockers: strings(item.get("blockers")),
            })
            .collect(),
        messages: list("messages")
            .iter()
            .map(|item| SummaryMessage {
                message_id: text(item, "messageId").unwrap(),
                from: text(item, "from").unwrap(),
                body: text(item, "body").unwrap(),
                state: text(item, "state").unwrap(),
            })
            .collect(),
        plans: list("plans")
            .iter()
            .map(|item| SummaryPlan {
                plan_id: text(item, "planId").unwrap(),
                title: text(item, "title").unwrap(),
                tier: text(item, "tier").unwrap(),
                state: text(item, "state").unwrap(),
                packages: number(item, "packages").unwrap(),
                signed_off: strings(item.get("signedOff")),
            })
            .collect(),
        integrations: list("integrations")
            .iter()
            .map(|item| SummaryIntegration {
                integration_id: text(item, "integrationId").unwrap(),
                branch: text(item, "branch").unwrap(),
                head_sha: opt(item, "headSha"),
            })
            .collect(),
        links: list("links")
            .iter()
            .map(|item| SummaryLink {
                ref_kind: text(item, "refKind").unwrap(),
                ref_id: text(item, "refId").unwrap(),
                external_id: text(item, "externalId").unwrap(),
                synced_state: text(item, "syncedState").unwrap(),
                wanted: opt(item, "wanted"),
                drift: matches!(item.get("drift"), Some(Value::Bool(true))),
                bound_agent_id: opt(item, "boundAgentId"),
            })
            .collect(),
        truncated: matches!(value.get("truncated"), Some(Value::Bool(true))),
        generated_at: text(value, "generatedAt").unwrap(),
    }
}

fn input(value: &Value) -> PromptInput {
    let kind = match text(value, "kind").unwrap().as_str() {
        "PM" => Kind::Pm,
        "Developer" => Kind::Developer,
        "Verifier" => Kind::Verifier,
        "Supervisor" => Kind::Supervisor,
        other => panic!("kind {other}"),
    };
    let flag = |key: &str| matches!(value.get(key), Some(Value::Bool(true)));
    PromptInput {
        role_name: text(value, "roleName").unwrap(),
        kind,
        agent_id: text(value, "agentId").unwrap(),
        wait_timeout_seconds: number(value, "waitTimeoutSeconds").unwrap(),
        role_prompt: opt(value, "rolePrompt"),
        restart_summary: value.get("restartSummary").map(summary),
        replacement_seed: opt(value, "replacementSeed"),
        worker_roles: match value.get("workerRoles") {
            Some(Value::Array(roles)) => Some(
                roles
                    .iter()
                    .map(|role| WorkerRole {
                        name: text(role, "name").unwrap(),
                        kind: text(role, "kind").unwrap(),
                    })
                    .collect(),
            ),
            _ => None,
        },
        is_architect: flag("isArchitect"),
        architect: value.get("architect").map(|a| ArchitectInput {
            role: text(a, "role").unwrap(),
            high_risk_triggers: strings(a.get("highRiskTriggers")),
            design_note: None,
        }),
        is_operator: flag("isOperator"),
        operator: value.get("operator").map(|o| OperatorInput {
            role: text(o, "role").unwrap(),
            auto_approve: strings(o.get("autoApprove")),
        }),
        is_researcher: flag("isResearcher"),
        researcher: value.get("researcher").map(|r| ResearcherInput {
            role: text(r, "role").unwrap(),
            output_dir: text(r, "outputDir").unwrap(),
            user_agent: text(r, "userAgent").unwrap(),
        }),
        prompt_relay_enabled: matches!(
            value.get("promptRelay").and_then(|p| p.get("enabled")),
            Some(Value::Bool(true))
        ),
        nexora: value.get("nexora").map(|n| NexoraInput {
            track: leak(text(n, "track").unwrap()),
            default_action: leak(text(n, "defaultAction").unwrap()),
        }),
    }
}

#[test]
fn every_prompt_variant_equals_nodes_text() {
    let mut inputs: HashMap<String, (PromptInput, Option<String>)> = HashMap::new();
    for file in ["prompts-corpus.json", "prompts-built.json"] {
        for case in items(&read_json(file)) {
            let name = text(case, "name").unwrap();
            let value = case.get("input").unwrap();
            inputs.insert(name, (input(value), opt(case, "fixture")));
        }
    }
    let expected = read_json("prompts-expected.json");
    let overlays = prompt_overlays();
    assert!(
        !overlays.is_empty(),
        "the architect reference overlays were not read"
    );
    let mut failures = Vec::new();
    let mut compared = 0;
    for want in items(&expected) {
        let name = text(want, "name").unwrap();
        let (prompt_input, _) = inputs
            .get(&name)
            .unwrap_or_else(|| panic!("no input for {name}"));
        compared += 1;
        match (build_role_prompt(prompt_input), text(want, "error")) {
            (Ok(prompt), None) if prompt_input.kind == Kind::Pm => {
                // The PM prompt is Rust-owned text (pm_reference.txt), so its frozen bytes, hash and golden file are not
                // compared: an edit of the prompt would need every one of them regenerated. What stays checked is that
                // Node built it too and that it is addressed to this agent and under the size cap.
                if !prompt.contains(&prompt_input.agent_id) || prompt.len() > MAX_PROMPT_BYTES {
                    failures.push(format!(
                        "{name}: the PM prompt lacks the agent id or is over the cap"
                    ));
                }
            }
            (Ok(prompt), None) if overlays.get(PROMPTS_FIXTURE, &name).is_some() => {
                // Rust deliberately differs from Node's text here (see tests/divergences/README.md): the overlay holds the
                // whole built prompt, compared with no stripping.
                let overlay = overlays.expected(PROMPTS_FIXTURE, &name).unwrap();
                if overlay["text"].as_str() != Some(prompt.as_str())
                    || overlay["bytes"].as_u64() != Some(prompt.len() as u64)
                    || overlay["sha256"].as_str() != Some(sha256_hex(prompt.as_bytes()).as_str())
                {
                    failures.push(format!("{name}: differs from its overlay"));
                }
            }
            (Ok(prompt), None) => {
                // The Operator reference carries one Rust-only sentence (restart through the restart kind); Node's
                // frozen text, hash and golden file lack it, so compare without it.
                let prompt = prompt.replace(RUST_ONLY_OPERATOR_SENTENCE, "");
                if let Some(fixture) = text(want, "fixture") {
                    let golden = std::fs::read_to_string(format!(
                        "{}/../../testdata/fixtures/prompts-off/{fixture}.txt",
                        env!("CARGO_MANIFEST_DIR")
                    ))
                    .unwrap();
                    if prompt != golden {
                        failures.push(format!(
                            "{name}: differs from the golden file {fixture}.txt"
                        ));
                    }
                }
                if prompt.len() != number(want, "bytes").unwrap() as usize
                    || sha256_hex(prompt.as_bytes()) != text(want, "sha256").unwrap()
                {
                    failures.push(format!(
                        "{name}: text differs from Node's (run --show-prompt {name})"
                    ));
                }
            }
            (Err(_), Some(error)) => assert_eq!(error, "RangeError", "{name}"),
            (Ok(_), Some(error)) => {
                failures.push(format!("{name}: Node refused ({error}), Rust built it"))
            }
            (Err(error), None) => {
                failures.push(format!("{name}: Node built it, Rust refused: {error}"))
            }
        }
    }
    assert!(compared >= 150, "only {compared} variants");
    assert!(
        failures.is_empty(),
        "{} of {compared} differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

const PROMPTS_FIXTURE: &str = "prompts-expected.json";

fn prompt_overlays() -> capstan_parity_overlay::Overlays {
    capstan_parity_overlay::load(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/divergences"),
        &parity_file(""),
        &|fixture| {
            let value = capstan_wire::js::parse(
                &std::fs::read_to_string(fixture).map_err(|e| e.to_string())?,
            )
            .map_err(|e| format!("{e:?}"))?;
            Ok(items(&value)
                .iter()
                .filter_map(|case| text(case, "name"))
                .collect())
        },
    )
    .unwrap()
}

const RUST_ONLY_OPERATOR_SENTENCE: &str = "To restart the controller always use the restart form: a command that runs `cstan stop` or `cstan restart` is refused, because the controller that runs the command would stop itself and the rest of the command would never run. ";

#[test]
fn the_constants_are_node_s() {
    assert_eq!(CSTAN_ALLOW_RULE, "Bash(cstan *)");
    assert_eq!(MAX_PROMPT_BYTES, 163_840);
}
