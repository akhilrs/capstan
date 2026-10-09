//! The Rust parsers, checks and builders against what Node computes on the same inputs (rust/crates/herdr/tests/parity,
//! written by test/herdr-parity-export.ts).

mod common;

use capstan_herdr::adapter::validate;
use capstan_herdr::api::{
    CapturedPrompt, HerdrError, HerdrOutput, HerdrRunner, PromptAnswer, RelayOption, RunOptions,
    StubRunner,
};
use capstan_herdr::claude_args::{
    build_agent_environment, claude_arguments, ClaudeRoleSettings, McpServer,
};
use capstan_herdr::hosts::{codex_arguments, omp_arguments, toml_string, HostRoleSettings};
use capstan_herdr::naming;
use capstan_herdr::process_activity::{
    parse_proc_stat, parse_ps_table, parse_ps_time, tool_processes, HerdrProcessProbe, ProcEntry,
    ProcIo, ProcessActivityTracker, ToolProcessWalker,
};
use capstan_herdr::prompt_relay::{check_answer, prompt_hash, relay_text_problem, HashInput};
use capstan_herdr::runner::{describe_output, failure_of, herdr_environment, run_json};
use capstan_herdr::screen::*;
use common::*;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

fn s(value: &Value) -> &str {
    value.as_str().expect("a string")
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../test/fixtures")
}

fn options_json(options: &[RelayOption]) -> Value {
    Value::Array(
        options
            .iter()
            .map(|o| {
                json!({
                    "number": o.number,
                    "text": o.text,
                    "acceptsText": o.accepts_text,
                    "widensPermissions": o.widens_permissions,
                })
            })
            .collect(),
    )
}

fn trust_json(parsed: Option<TrustDialog>) -> Value {
    match parsed {
        None => Value::Null,
        Some(TrustDialog::WrappedPath) => json!({"kind": "wrapped_path"}),
        Some(TrustDialog::Dialog {
            path,
            options,
            selected_index,
            confirm_is_last_line,
        }) => json!({
            "kind": "dialog",
            "path": path,
            "options": options.iter().map(|o| json!({"text": o.text, "selected": o.selected})).collect::<Vec<_>>(),
            "selectedIndex": selected_index.map_or(Value::Null, |i| json!(i)),
            "confirmIsLastLine": confirm_is_last_line,
        }),
    }
}

/// The view `screenView` of the exporter builds.
fn screen_view(content: &str) -> Value {
    let plain = strip_ansi(content);
    let kinds = ["claude", "codex", "omp", "shell", "other"];
    let mut input = Map::new();
    let mut blocker = Map::new();
    for kind in kinds {
        input.insert(kind.into(), option_text(extract_input_line(kind, content)));
        blocker.insert(
            kind.into(),
            json!(classify_input_blocker(kind, content).as_str()),
        );
    }
    let mut trust = Map::new();
    for kind in ["claude", "codex", "omp"] {
        trust.insert(
            kind.into(),
            trust_json(parse_trust_dialog_of(kind, content)),
        );
    }
    let host = parse_host_prompt("claude", content);
    let dialog = parse_blocking_dialog("claude", content);
    let pane = |text: &str, options: &[RelayOption], dialog: bool| {
        prompt_hash(&HashInput {
            agent_id: "dev",
            pane_id: "w1:p1",
            host_kind: "claude",
            text,
            options,
            dialog,
        })
    };
    json!({
        "bytes": content.len(),
        "sha": sha_hex(content),
        "stripped": {"length": plain.encode_utf16().count(), "sha": sha_hex(&plain)},
        "fresh": fresh_prompt_ready(&plain),
        "freshRaw": fresh_prompt_ready(content),
        "input": input,
        "blocker": blocker,
        "trust": trust,
        "footer": option_text(prompt_footer(content)),
        "hostPrompt": host.as_ref().map_or(Value::Null, |h| json!({
            "text": h.text,
            "selectedIndex": h.selected_index,
            "options": options_json(&h.options),
            "sha": pane(&h.text, &h.options, false),
        })),
        "hostPromptCodex": if parse_host_prompt("codex", content).is_some() { json!(1) } else { Value::Null },
        "dialog": dialog.as_ref().map_or(Value::Null, |text| json!({
            "text": text,
            "sha": pane(text, &[], true),
        })),
    })
}

fn check_screen_case(case: &Value, content: &str) {
    let name = s(&case["name"]);
    let view = screen_view(content);
    let Value::Object(expected) = case else {
        panic!()
    };
    let Value::Object(actual) = &view else {
        panic!()
    };
    for (key, value) in expected {
        if key == "name" || key == "content" {
            continue;
        }
        expect_eq(&format!("{name}: {key}"), &actual[key], value);
    }
    assert_eq!(
        actual.len(),
        expected.len()
            - if expected.contains_key("content") {
                2
            } else {
                1
            },
        "{name}: the same fields"
    );
}

#[test]
fn every_fixture_screen_parses_like_node() {
    let screen = load("screen.json");
    let fixtures = screen["fixtures"].as_array().unwrap();
    assert!(fixtures.len() > 30, "the fixture captures are all there");
    for case in fixtures {
        let content = std::fs::read_to_string(fixtures_dir().join(s(&case["name"])))
            .unwrap_or_else(|e| panic!("{}: {e}", case["name"]));
        check_screen_case(case, &content);
    }
}

#[test]
fn synthetic_screens_parse_like_node() {
    let screen = load("screen.json");
    let cases = screen["synthetic"].as_array().unwrap();
    assert!(cases.len() > 60);
    for case in cases {
        check_screen_case(case, s(&case["content"]));
    }
}

#[test]
fn text_field_wording_is_the_recorded_one() {
    let screen = load("screen.json");
    for pair in screen["textFieldWording"].as_array().unwrap() {
        assert_eq!(text_field_wording(s(&pair[0])), Some(s(&pair[1])));
    }
    assert_eq!(text_field_wording("Maybe"), None);
}

#[test]
fn trust_texts_name_the_two_hosts() {
    assert_eq!(trust_texts("claude"), Some((TRUST_YES, TRUST_NO)));
    assert_eq!(
        trust_texts("codex"),
        Some((CODEX_TRUST_YES, CODEX_TRUST_NO))
    );
    assert_eq!(trust_texts("omp"), None);
}

#[test]
fn relay_checks_match_node() {
    let relay = load("relay.json");
    for case in relay["texts"].as_array().unwrap() {
        expect_eq(
            &format!("relay text {:?}", case["text"]),
            &option_text(relay_text_problem(s(&case["text"]))),
            &case["problem"],
        );
    }
    let options: Vec<RelayOption> = relay["prompt"]["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| RelayOption {
            number: o["number"].as_i64().unwrap(),
            text: s(&o["text"]).to_string(),
            accepts_text: o["acceptsText"].as_bool().unwrap(),
            widens_permissions: o["widensPermissions"].as_bool().unwrap(),
        })
        .collect();
    let prompt = CapturedPrompt {
        agent_id: "dev".into(),
        pane_id: "w1:p1".into(),
        host_kind: "claude".into(),
        text: s(&relay["prompt"]["text"]).to_string(),
        options,
        prompt_sha: s(&relay["promptSha"]).to_string(),
        dialog: false,
    };
    for case in relay["answers"].as_array().unwrap() {
        let a = &case["answer"];
        let answer = match s(&a["kind"]) {
            "esc" => PromptAnswer::Esc,
            "option" => PromptAnswer::Option {
                number: a["number"].as_i64().unwrap(),
            },
            _ => PromptAnswer::Text {
                number: a["number"].as_i64().unwrap(),
                text: s(&a["text"]).to_string(),
            },
        };
        expect_eq(
            &format!("answer {a}"),
            &check_answer(&prompt, &answer).map_or(Value::Null, |r| json!(r.as_str())),
            &case["refusal"],
        );
    }
    for case in relay["hashes"].as_array().unwrap() {
        let input = &case["input"];
        let options: Vec<RelayOption> = input["options"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| RelayOption {
                number: o["number"].as_i64().unwrap(),
                text: s(&o["text"]).to_string(),
                accepts_text: o["acceptsText"].as_bool().unwrap(),
                widens_permissions: o["widensPermissions"].as_bool().unwrap(),
            })
            .collect();
        let sha = prompt_hash(&HashInput {
            agent_id: s(&input["agentId"]),
            pane_id: s(&input["paneId"]),
            host_kind: s(&input["hostKind"]),
            text: s(&input["text"]),
            options: &options,
            dialog: input["dialog"].as_bool().unwrap_or(false),
        });
        assert_eq!(sha, s(&case["sha"]), "hash of {input}");
    }
}

fn result_json(result: Result<Value, String>) -> Value {
    match result {
        Ok(value) => json!({"ok": value}),
        Err(message) => json!({"error": message}),
    }
}

fn adapter_message(result: capstan_herdr::api::AdapterResult<Value>) -> Value {
    result_json(result.map_err(|e| e.message().to_string()))
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|v| s(v).to_string())
        .collect()
}

fn role_of(value: &Value) -> ClaudeRoleSettings {
    ClaudeRoleSettings {
        model: value["model"].as_str().map(str::to_string),
        permission_mode: s(&value["permissionMode"]).to_string(),
        allow: strings(&value["allow"]),
        deny: strings(&value["deny"]),
        hooks: s(&value["hooks"]).to_string(),
        mcp: value["mcp"]
            .as_array()
            .map(|servers| {
                servers
                    .iter()
                    .map(|m| McpServer {
                        name: s(&m["name"]).to_string(),
                        command: s(&m["command"]).to_string(),
                        args: strings(&m["args"]),
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

#[test]
fn validation_and_argument_builders_match_node() {
    let v = load("validate.json");
    for group in v["patterns"].as_array().unwrap() {
        let name = s(&group["name"]);
        for case in group["cases"].as_array().unwrap() {
            let value = s(&case[0]);
            let actual = match name {
                "branch" => validate::BRANCH_PATTERN.is_match(value),
                "workspace" => validate::WORKSPACE_PATTERN.is_match(value),
                "pane" => validate::PANE_PATTERN.is_match(value),
                "tab" => validate::TAB_PATTERN.is_match(value),
                "simple" => validate::SIMPLE_VALUE.is_match(value),
                "envKey" => validate::ENVIRONMENT_KEY.is_match(value),
                "token" => validate::TOKEN_NAME.is_match(value),
                other => panic!("pattern {other}"),
            };
            assert_eq!(json!(actual), case[1], "{name} on {value:?}");
        }
    }
    for case in v["agentNames"].as_array().unwrap() {
        assert_eq!(
            json!(validate::is_agent_name(s(&case[0]))),
            case[1],
            "{:?}",
            case[0]
        );
    }
    for case in v["labels"].as_array().unwrap() {
        let value = s(&case["value"]);
        let actual = match validate::require_label(value) {
            Ok(label) => json!({"ok": label}),
            Err(e) => json!({"error": e.message()}),
        };
        expect_eq(&format!("label {value:?}"), &actual, &case["result"]);
    }
    for case in v["safeText"].as_array().unwrap() {
        let value = s(&case["value"]);
        assert_eq!(
            validate::is_safe_text(value),
            case["safe"].as_bool().unwrap(),
            "safe {value:?}"
        );
        assert_eq!(
            validate::command_start(value),
            case["command"].as_bool().unwrap(),
            "command {value:?}"
        );
    }
    for case in v["shellQuote"].as_array().unwrap() {
        assert_eq!(validate::shell_quote(s(&case[0])), s(&case[1]));
    }
    for case in v["slugs"].as_array().unwrap() {
        assert_eq!(
            naming::project_slug(s(&case[0])),
            s(&case[1]),
            "slug of {:?}",
            case[0]
        );
    }
    for case in v["displayNames"].as_array().unwrap() {
        let configured = case["configured"].as_str();
        assert_eq!(
            naming::project_display_name(configured, s(&case["root"])),
            s(&case["name"]),
            "display name {case}"
        );
    }
    for case in v["agentNamesFor"].as_array().unwrap() {
        let actual = result_json(
            naming::herdr_agent_name(s(&case["slug"]), s(&case["id"])).map(Value::String),
        );
        expect_eq(&format!("agent name {case}"), &actual, &case["result"]);
    }
    for case in v["workspaceLabels"].as_array().unwrap() {
        assert_eq!(
            naming::workspace_label(s(&case[0]), s(&case[1])),
            s(&case[2])
        );
    }
    for case in v["claude"].as_array().unwrap() {
        let role = role_of(&case["role"]);
        let actual =
            adapter_message(claude_arguments(&role, case["prompt"].as_str()).map(|a| json!(a)));
        expect_eq(
            &format!("claude {}", case["name"]),
            &actual,
            &case["result"],
        );
    }
    for case in v["environments"].as_array().unwrap() {
        let base: HashMap<String, String> = case["base"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), s(v).to_string()))
            .collect();
        let extras: Vec<(String, String)> = case["extras"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (s(&p[0]).to_string(), s(&p[1]).to_string()))
            .collect();
        let actual = adapter_message(
            build_agent_environment(&base, &extras, &strings(&case["pass"]))
                .map(|m| json!(m.into_iter().collect::<BTreeMap<_, _>>())),
        );
        expect_eq(
            &format!("environment {}", case["name"]),
            &actual,
            &case["result"],
        );
    }
    for case in v["tomlStrings"].as_array().unwrap() {
        assert_eq!(toml_string(s(&case[0])), s(&case[1]));
    }
    for case in v["hosts"].as_array().unwrap() {
        let role = HostRoleSettings {
            model: case["model"].as_str().map(str::to_string),
        };
        let worktree = case["worktree"].as_str().map(PathBuf::from);
        let codex = adapter_message(
            codex_arguments(&role, s(&case["prompt"]), worktree.as_deref()).map(|a| json!(a)),
        );
        // A path that does not exist names the OS error in Node and in Rust; only the text before it is the contract.
        expect_eq(&format!("codex {case}"), &codex, &case["codex"]);
        let omp = adapter_message(omp_arguments(&role, "/tmp/p.md").map(|a| json!(a)));
        expect_eq(&format!("omp {case}"), &omp, &case["omp"]);
    }
    let missing = codex_arguments(
        &HostRoleSettings { model: None },
        "p",
        Some(std::path::Path::new("/definitely/not/here")),
    );
    assert!(
        missing.is_err(),
        "a worktree that does not exist is refused"
    );
    assert!(v["hostsMissingWorktree"]["error"].is_string());
    let huge = adapter_message(
        codex_arguments(
            &HostRoleSettings { model: None },
            &"x".repeat(121 * 1024),
            None,
        )
        .map(|a| json!(a)),
    );
    expect_eq("huge prompt", &huge, &v["hostsHuge"]);
}

fn output(value: &Value) -> HerdrOutput {
    HerdrOutput {
        code: value["code"].as_i64().unwrap() as i32,
        stdout: s(&value["stdout"]).to_string(),
        stderr: s(&value["stderr"]).to_string(),
    }
}

#[test]
fn runner_helpers_match_node() {
    let r = load("runner.json");
    for case in r["describe"].as_array().unwrap() {
        assert_eq!(
            describe_output(s(&case[0])),
            s(&case[1]),
            "describe {:?}",
            case[0]
        );
    }
    for case in r["results"].as_array().unwrap() {
        let args = strings(&case["args"]);
        let outcome = output(&case["outcome"]);
        let failure = failure_of(&args, &outcome);
        expect_eq(
            &format!("failure {}", case["name"]),
            &json!({"code": failure.code, "message": failure.message}),
            &case["failure"],
        );
        let stub = StubRunner::new();
        stub.push(Ok(outcome));
        let actual = match run_json(&stub, &args, &RunOptions::default()) {
            Ok(map) => json!({"ok": map}),
            Err(e) => {
                json!({"error": {"name": "HerdrError", "message": e.message, "code": e.code}})
            }
        };
        // Node's error may be a plain HerdrError; its `name` is HerdrError as well.
        expect_eq(
            &format!("run_json {}", case["name"]),
            &actual,
            &case["json"],
        );
    }
    let base: HashMap<String, String> = r["environment"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), s(v).to_string()))
        .collect();
    let input: HashMap<String, String> = [
        ("HERDR_SESSION", "x"),
        ("HERDR_PANE", "y"),
        ("HOME", "/h"),
        ("herdr_lower", "z"),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    assert_eq!(herdr_environment(&input), base);
}

fn entry_json(entry: &ProcEntry) -> Value {
    json!({
        "pid": entry.pid,
        "ppid": entry.ppid,
        "comm": entry.comm,
        "cpuMs": number(entry.cpu_ms),
        "startKey": entry.start_key,
    })
}

fn entry_of(value: &Value) -> ProcEntry {
    ProcEntry {
        pid: value["pid"].as_i64().unwrap(),
        ppid: value["ppid"].as_i64().unwrap(),
        comm: s(&value["comm"]).to_string(),
        cpu_ms: value["cpuMs"].as_f64().unwrap(),
        start_key: s(&value["startKey"]).to_string(),
    }
}

struct ScenarioIo {
    scenario: Value,
}

impl ScenarioIo {
    fn fail(&self, key: &str) -> io::Result<()> {
        match self.scenario["failures"][key].as_str() {
            None => Ok(()),
            Some("ENOENT") => Err(io::Error::from_raw_os_error(libc::ENOENT)),
            Some("ESRCH") => Err(io::Error::from_raw_os_error(libc::ESRCH)),
            Some("EMFILE") => Err(io::Error::from_raw_os_error(libc::EMFILE)),
            Some("EACCES") => Err(io::Error::from_raw_os_error(libc::EACCES)),
            Some(other) => panic!("failure {other}"),
        }
    }
}

impl ProcIo for ScenarioIo {
    fn threads(&self, pid: i64) -> io::Result<Vec<String>> {
        self.fail(&format!("threads/{pid}"))?;
        match self.scenario["threads"][pid.to_string()].as_array() {
            Some(list) => Ok(strings(&Value::Array(list.clone()))),
            None => Err(io::Error::from_raw_os_error(libc::ENOENT)),
        }
    }

    fn children(&self, pid: i64, thread: &str) -> io::Result<String> {
        self.fail(&format!("children/{pid}/{thread}"))?;
        match self.scenario["children"][format!("{pid}/{thread}")].as_str() {
            Some(list) => Ok(list.to_string()),
            None => Err(io::Error::from_raw_os_error(libc::ENOENT)),
        }
    }

    fn stat(&self, pid: i64) -> io::Result<String> {
        self.fail(&format!("stat/{pid}"))?;
        match self.scenario["stats"][pid.to_string()].as_str() {
            Some(text) => Ok(text.to_string()),
            None => Err(io::Error::from_raw_os_error(libc::ENOENT)),
        }
    }
}

#[test]
fn process_parsers_match_node() {
    let p = load("process.json");
    for case in p["procStats"].as_array().unwrap() {
        let actual = parse_proc_stat(
            case["pid"].as_i64().unwrap(),
            s(&case["content"]),
            case["ticks"].as_f64().unwrap(),
        )
        .map_or(Value::Null, |e| entry_json(&e));
        expect_eq(
            &format!("proc stat {}", case["pid"]),
            &actual,
            &case["result"],
        );
    }
    for case in p["procStatTicks"].as_array().unwrap() {
        let actual = parse_proc_stat(10, s(&case["content"]), case["ticks"].as_f64().unwrap())
            .map_or(Value::Null, |e| entry_json(&e));
        expect_eq("proc stat ticks", &actual, &case["result"]);
    }
    for case in p["psTimes"].as_array().unwrap() {
        let actual = parse_ps_time(s(&case[0])).map_or(Value::Null, number);
        expect_eq(&format!("ps time {:?}", case[0]), &actual, &case[1]);
    }
    for case in p["psTables"].as_array().unwrap() {
        let entries: Vec<Value> = parse_ps_table(s(&case["text"]))
            .iter()
            .map(entry_json)
            .collect();
        expect_eq(
            &format!("ps table {:?}", case["text"]),
            &Value::Array(entries),
            &case["entries"],
        );
    }
    for case in p["tables"].as_array().unwrap() {
        let table: Vec<ProcEntry> = case["table"]
            .as_array()
            .unwrap()
            .iter()
            .map(entry_of)
            .collect();
        let counted: Vec<Value> = tool_processes(&table, case["shellPid"].as_i64().unwrap())
            .iter()
            .map(entry_json)
            .collect();
        expect_eq(
            &format!("tool processes {}", case["name"]),
            &Value::Array(counted),
            &case["counted"],
        );
    }
}

#[test]
fn the_tool_process_walker_behaves_like_node() {
    let p = load("process.json");
    for scenario in p["walkers"].as_array().unwrap() {
        let clock = Arc::new(AtomicU64::new(0));
        let reader = clock.clone();
        let walker = ToolProcessWalker::new(
            Arc::new(ScenarioIo {
                scenario: scenario.clone(),
            }),
            Arc::new(move || reader.load(Ordering::SeqCst)),
            scenario["retryMs"].as_u64().unwrap(),
            scenario["self"].as_i64().unwrap(),
        );
        for (step, expected) in scenario["steps"]
            .as_array()
            .unwrap()
            .iter()
            .zip(scenario["results"].as_array().unwrap())
        {
            clock.store(step["now"].as_u64().unwrap(), Ordering::SeqCst);
            let actual = walker
                .read(step["shellPid"].as_i64().unwrap())
                .map_or(Value::Null, |list| {
                    Value::Array(list.iter().map(entry_json).collect())
                });
            expect_eq(
                &format!("walker {} at {}", scenario["name"], step["now"]),
                &actual,
                expected,
            );
        }
    }
}

#[test]
fn the_activity_tracker_behaves_like_node() {
    let p = load("process.json");
    for run in p["trackers"].as_array().unwrap() {
        let mut tracker = ProcessActivityTracker::new();
        let steps = run["steps"].as_array().unwrap();
        let agents: Vec<&str> = {
            let mut seen: Vec<&str> = Vec::new();
            for step in steps {
                let id = s(&step["agent"]);
                if !seen.contains(&id) {
                    seen.push(id);
                }
            }
            seen
        };
        for (index, step) in steps.iter().enumerate() {
            let processes: Vec<ProcEntry> = step["processes"]
                .as_array()
                .unwrap()
                .iter()
                .map(entry_of)
                .collect();
            tracker.record(s(&step["agent"]), &processes, step["at"].as_u64().unwrap());
            let state: Vec<Value> = agents
                .iter()
                .map(|id| {
                    json!([
                        id,
                        tracker
                            .last_child_activity(id)
                            .map_or(Value::Null, |v| json!(v))
                    ])
                })
                .collect();
            expect_eq(
                &format!("tracker {} step {index}", run["name"]),
                &Value::Array(state),
                &run["states"][index],
            );
        }
        tracker.forget("a");
        let mut activity: Vec<(String, u64)> = tracker
            .activity()
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        activity.sort();
        let last = &run["states"][steps.len()];
        assert_eq!(
            json!(tracker
                .last_child_activity("a")
                .map_or(Value::Null, |v| json!(v))),
            last[0][1]
        );
        let mut expected: Vec<(String, u64)> = last[1][1]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (s(&p[0]).to_string(), p[1].as_u64().unwrap()))
            .collect();
        expected.sort();
        assert_eq!(activity, expected, "tracker {} activity", run["name"]);
    }
}

#[test]
fn the_probe_reads_the_shell_pid_and_counts_tool_processes_like_node() {
    let p = load("process.json");
    for case in p["probes"].as_array().unwrap() {
        let table: Vec<ProcEntry> = case["table"]
            .as_array()
            .unwrap()
            .iter()
            .map(entry_of)
            .collect();
        let runner = Arc::new(StubRunner::new());
        runner.push(Ok(output(&case["outcome"])));
        let probe = HerdrProcessProbe::with_table_reader(
            runner.clone(),
            Arc::new(move || Ok(table.clone())),
        );
        let actual = match probe.sample_detailed("w1:p1") {
            Ok(sample) => {
                json!({"ok": sample.processes.iter().map(entry_json).collect::<Vec<_>>()})
            }
            Err(e) => json!({"error": e.message}),
        };
        expect_eq(&format!("probe {}", case["name"]), &actual, &case["result"]);
        let calls: Vec<Vec<String>> = runner.calls();
        let expected: Vec<Vec<String>> = case["calls"]
            .as_array()
            .unwrap()
            .iter()
            .map(strings)
            .collect();
        assert_eq!(calls, expected, "probe {} calls", case["name"]);
    }
}

#[test]
fn reading_the_real_proc_table_finds_this_process() {
    if !cfg!(target_os = "linux") {
        return;
    }
    let table = capstan_herdr::process_activity::read_proc_table().expect("/proc reads");
    let me = i64::from(std::process::id());
    assert!(
        table.iter().any(|e| e.pid == me),
        "the test process is in the table"
    );
    let walker = ToolProcessWalker::system();
    // Our own children list reads (or the kernel has none); either way the walker answers without failing.
    let _ = walker.read(me);
}

#[test]
fn a_failed_herdr_call_is_the_error_the_runner_made() {
    let stub = StubRunner::new();
    stub.push(Err(HerdrError::new("timeout", "herdr pane timed out")));
    let error = run_json(&stub, &["pane".to_string()], &RunOptions::default()).unwrap_err();
    assert_eq!(
        (error.code.as_str(), error.message.as_str()),
        ("timeout", "herdr pane timed out")
    );
    let _: &dyn HerdrRunner = &stub;
}
