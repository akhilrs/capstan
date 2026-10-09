//! The Rust adapter driven by the answers the Node adapter got from the fake Herdr (rust/crates/herdr/tests/parity/
//! adapter.json, written by test/herdr-parity-export.ts): it must make the same herdr calls (argv, in order), tell its
//! caller the same, call its hooks and the key log the same way, and wait the same number of poll intervals.

mod common;

use capstan_herdr::adapter::{Adapter, AdapterOptions};
use capstan_herdr::api::*;
use common::*;
use regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

fn s(value: &Value) -> &str {
    value.as_str().expect("a string")
}

/// Replaces the temporary root and the random shell directory names the way the exporter does.
fn normalise(value: &Value, root: &str) -> Value {
    static SHELL: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new("capstan-shell-[A-Za-z0-9]{6}").unwrap());
    match value {
        Value::String(text) => Value::String(
            SHELL
                .replace_all(&text.replace(root, "<root>"), "capstan-shell-XXXXXX")
                .into_owned(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(|v| normalise(v, root)).collect()),
        Value::Object(map) => {
            let mut out = Map::new();
            for (k, v) in map {
                out.insert(k.clone(), normalise(v, root));
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

/// The recorded arguments name the temporary root as `<root>`; the replay runs under its own.
fn denormalise(value: &Value, root: &str) -> Value {
    match value {
        Value::String(text) => Value::String(text.replace("<root>", root)),
        Value::Array(items) => Value::Array(items.iter().map(|v| denormalise(v, root)).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), denormalise(v, root)))
                .collect(),
        ),
        other => other.clone(),
    }
}

struct Replay {
    root: String,
    queue: Mutex<VecDeque<Value>>,
    mismatches: Mutex<Vec<String>>,
}

impl Replay {
    fn load(&self, calls: &[Value]) {
        *self.queue.lock().unwrap() = calls.iter().cloned().collect();
    }

    fn problem(&self, text: String) -> HerdrError {
        self.mismatches.lock().unwrap().push(text.clone());
        HerdrError::new("replay", text)
    }
}

impl HerdrRunner for Replay {
    fn run(&self, args: &[String], _options: &RunOptions) -> Result<HerdrOutput, HerdrError> {
        let Some(call) = self.queue.lock().unwrap().pop_front() else {
            return Err(self.problem(format!("unexpected call {args:?}")));
        };
        let actual = normalise(&json!(args), &self.root);
        if actual != call["argv"] {
            return Err(self.problem(format!(
                "call differs\n  rust: {actual}\n  node: {}",
                call["argv"]
            )));
        }
        for dir in call["dirs"].as_array().unwrap() {
            std::fs::create_dir_all(Path::new(&self.root).join(s(dir))).unwrap();
        }
        // The fake's `onRun`: starting the prepared shell removed its private directory, when the scenario let it.
        if call["rcRemoved"] == true {
            let rc = Regex::new("--rcfile '([^']+)'").unwrap();
            if let Some(found) = args.get(3).and_then(|a| rc.captures(a)) {
                let rc_file = PathBuf::from(&found[1]);
                let _ = std::fs::remove_dir_all(rc_file.parent().unwrap());
            }
        }
        let response = &call["response"];
        Ok(HerdrOutput {
            code: response["code"].as_i64().unwrap() as i32,
            stdout: s(&response["stdout"]).replace("<root>", &self.root),
            stderr: s(&response["stderr"]).replace("<root>", &self.root),
        })
    }
}

fn error_json(error: &AdapterError) -> Value {
    let mut map = Map::new();
    map.insert("name".into(), json!(error.name()));
    map.insert("message".into(), json!(error.message()));
    if let Some(code) = error.herdr_code() {
        map.insert("code".into(), json!(code));
    }
    if let AdapterError::InputUnreadable { blocker, .. } = error {
        map.insert("blocker".into(), json!(blocker.as_str()));
    }
    Value::Object(map)
}

fn role_of(value: &Value) -> PaneRole {
    if s(value) == "PM" {
        PaneRole::Pm
    } else {
        PaneRole::Worker
    }
}

fn env_of(value: &Value) -> BTreeMap<String, String> {
    value
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), s(v).to_string()))
        .collect()
}

fn prompt_json(prompt: &CapturedPrompt) -> Value {
    let mut map = Map::new();
    map.insert("agentId".into(), json!(prompt.agent_id));
    map.insert("paneId".into(), json!(prompt.pane_id));
    map.insert("hostKind".into(), json!(prompt.host_kind));
    map.insert("text".into(), json!(prompt.text));
    map.insert(
        "options".into(),
        Value::Array(
            prompt
                .options
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
        ),
    );
    map.insert("promptSha".into(), json!(prompt.prompt_sha));
    if prompt.dialog {
        map.insert("dialog".into(), json!(true));
    }
    Value::Object(map)
}

fn entry_json(entry: Option<PaneEntry>) -> Value {
    let Some(entry) = entry else {
        return Value::Null;
    };
    let mut map = Map::new();
    map.insert("role".into(), json!(entry.role.as_str()));
    map.insert(
        "phase".into(),
        json!(match entry.phase {
            PanePhase::Fresh => "fresh",
            PanePhase::Prepared => "prepared",
            PanePhase::Started => "started",
            PanePhase::Tainted => "tainted",
        }),
    );
    map.insert("kind".into(), json!(entry.kind));
    if let Some(agent) = entry.agent {
        map.insert("agent".into(), json!(agent));
    }
    if let Some(path) = entry.worktree_path {
        map.insert("worktreePath".into(), json!(path));
    }
    if let Some(id) = entry.workspace_id {
        map.insert("workspaceId".into(), json!(id));
    }
    Value::Object(map)
}

/// Performs one recorded operation on the adapter and returns the JSON Node's result has.
fn perform(
    adapter: &Adapter,
    op: &str,
    args: &Value,
    events: &Mutex<Vec<String>>,
) -> AdapterResult<Value> {
    let text = |key: &str| args[key].as_str().unwrap_or("");
    let optional = |key: &str| args[key].as_str();
    let log = |entry: &KeyLogEntry| events.lock().unwrap().push(format!("key:{}", entry.key));
    let before = |label: &'static str, key: &'static str| {
        let throw = args[key] == "throw";
        move || -> HookResult {
            events.lock().unwrap().push(label.to_string());
            if throw {
                Err(HookError::new("StaleActionError", "stale"))
            } else {
                Ok(())
            }
        }
    };
    let number = |key: &str| match &args[key] {
        Value::Null => f64::INFINITY,
        other => other.as_f64().unwrap(),
    };
    Ok(match op {
        "version" => json!(adapter.version()?),
        "createWorktree" => {
            let created = adapter.create_worktree(CreateWorktreeInput {
                workspace_id: text("workspaceId"),
                branch: text("branch"),
                label: text("label"),
                base: optional("base"),
            })?;
            json!({"workspaceId": created.workspace_id, "paneId": created.pane_id, "path": created.path, "branch": created.branch})
        }
        "createWorkspace" => {
            let created = adapter.create_workspace(CreateWorkspaceInput {
                cwd: text("cwd"),
                label: text("label"),
                role: role_of(&args["role"]),
            })?;
            json!({"workspaceId": created.workspace_id, "paneId": created.pane_id, "tabId": created.tab_id})
        }
        "createTab" => {
            let created = adapter.create_tab(CreateTabInput {
                workspace_id: text("workspaceId"),
                cwd: text("cwd"),
                label: text("label"),
                role: role_of(&args["role"]),
            })?;
            json!({"tabId": created.tab_id, "paneId": created.pane_id})
        }
        "startAgent" => {
            let list: Vec<String> = args["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| s(v).to_string())
                .collect();
            let environment = args.get("environment").filter(|v| !v.is_null()).map(env_of);
            let status = adapter.start_agent(StartAgentInput {
                name: text("name"),
                kind: text("kind"),
                pane_id: text("paneId"),
                args: &list,
                timeout_ms: args["timeoutMs"].as_u64(),
                environment: environment.as_ref(),
            })?;
            json!({"status": match status {
                StartStatus::Started => "started",
                StartStatus::BlockedAtStartup => "blocked_at_startup",
            }})
        }
        "prepareShell" => {
            let environment = env_of(&args["environment"]);
            adapter.prepare_shell(PrepareShellInput {
                pane_id: text("paneId"),
                environment: &environment,
                timeout_ms: args["timeoutMs"].as_u64(),
            })?;
            Value::Null
        }
        "runInPane" => {
            adapter.run_in_pane(text("paneId"), text("command"))?;
            Value::Null
        }
        "guardedSend" => {
            let mut hook = before("beforeSend", "beforeSend");
            match adapter.guarded_send(GuardedSendInput {
                pane_id: text("paneId"),
                text: text("text"),
                before_send: &mut hook,
            })? {
                SendOutcome::Sent => json!({"sent": true}),
                SendOutcome::Deferred {
                    reason,
                    detail,
                    blocker,
                } => {
                    let mut map = Map::new();
                    map.insert("sent".into(), json!(false));
                    map.insert("reason".into(), json!(reason.as_str()));
                    if let Some(detail) = detail {
                        map.insert("detail".into(), json!(detail));
                    }
                    if let Some(blocker) = blocker {
                        map.insert("blocker".into(), json!(blocker.as_str()));
                    }
                    Value::Object(map)
                }
            }
        }
        "wakePm" => {
            let mut hook = before("beforeSend", "beforeSend");
            match adapter.wake_pm(WakePmInput {
                pane_id: text("paneId"),
                text: text("text"),
                before_send: &mut hook,
            })? {
                WakeOutcome::Sent => json!({"sent": true}),
                WakeOutcome::PmNotIdle => json!({"sent": false, "reason": "pm_not_idle"}),
                WakeOutcome::InputNotEmpty => json!({"sent": false, "reason": "input_not_empty"}),
            }
        }
        "clearAfterDeferral" => {
            let throw = args["discard"] == "throw";
            let mut discard = |text: &str| -> HookResult {
                events.lock().unwrap().push(format!("discard:{text}"));
                if throw {
                    Err(HookError::new("StaleActionError", "stale"))
                } else {
                    Ok(())
                }
            };
            let cleared = adapter.clear_after_deferral(ClearInput {
                pane_id: text("paneId"),
                deferred_for_ms: number("deferredForMs"),
                max_deferral_ms: number("maxDeferralMs"),
                discard: &mut discard,
                log: &log,
            })?;
            json!({"cleared": cleared.cleared, "text": cleared.text})
        }
        "answerTrustDialog" => {
            match adapter.answer_trust_dialog(TrustDialogInput {
                pane_id: text("paneId"),
                log: &log,
                timeout_ms: args["timeoutMs"].as_u64(),
            })? {
                DialogOutcome::Handled { keys } => json!({"handled": true, "keys": keys}),
                DialogOutcome::Unhandled { reason } => json!({"handled": false, "reason": reason}),
            }
        }
        "capturePrompt" => match adapter.capture_prompt(text("paneId"))? {
            CaptureOutcome::Captured(prompt) => {
                json!({"captured": true, "prompt": prompt_json(&prompt)})
            }
            CaptureOutcome::Refused(reason) => {
                json!({"captured": false, "reason": reason.as_str()})
            }
        },
        "answerPrompt" => {
            let a = &args["answer"];
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
            let mut hook = before("beforeType", "beforeType");
            match adapter.answer_prompt(AnswerPromptInput {
                pane_id: text("paneId"),
                prompt_sha: text("promptSha"),
                answer: &answer,
                before_type: &mut hook,
                log: &log,
            })? {
                RelayOutcome::Typed {
                    keys,
                    input_readable,
                } => {
                    let mut map = Map::new();
                    map.insert("typed".into(), json!(true));
                    map.insert("keys".into(), json!(keys));
                    if let Some(readable) = input_readable {
                        map.insert("inputReadable".into(), json!(readable));
                    }
                    Value::Object(map)
                }
                RelayOutcome::Refused { reason, keys } => {
                    json!({"typed": false, "reason": reason.as_str(), "keys": keys})
                }
            }
        }
        "interruptWorking" => json!({"sent": adapter.interrupt_working(text("paneId"), &log)?}),
        "readScreen" => json!(adapter.read_screen(
            text("paneId"),
            args["ansi"] == true,
            args["lines"].as_u64().map(|n| n as usize)
        )?),
        "readInput" => adapter
            .read_input(text("paneId"))?
            .map_or(Value::Null, Value::String),
        "agentState" => {
            let state = adapter.agent_state(text("name"))?;
            json!({"status": state.status, "paneId": state.pane_id, "kind": state.kind})
        }
        "paneState" => {
            let state = adapter.pane_state(text("paneId"))?;
            let mut map = Map::new();
            map.insert("status".into(), json!(state.status));
            if let Some(agent) = state.agent {
                map.insert("agent".into(), json!(agent));
            }
            Value::Object(map)
        }
        "paneEntry" => entry_json(adapter.pane_entry(text("paneId"))),
        "paneForAgent" => adapter
            .pane_for_agent(text("agentId"))
            .map_or(Value::Null, Value::String),
        "agentObservation" => json!(adapter.agent_observation(text("agentId"))?.as_str()),
        "forgetPane" => {
            adapter.forget_pane(text("paneId"));
            Value::Null
        }
        "notify" => {
            adapter.notify(text("title"), text("body"))?;
            Value::Null
        }
        "paneLayout" => {
            let layout = adapter.pane_layout(text("paneId"))?;
            json!({
                "tabId": layout.tab_id,
                "workspaceId": layout.workspace_id,
                "zoomed": layout.zoomed,
                "panes": layout.panes.iter().map(|p| json!({
                    "paneId": p.pane_id,
                    // A size Herdr did not give is NaN in Node, which JSON writes as null.
                    "width": if p.width == i64::MIN { Value::Null } else { json!(p.width) },
                    "height": if p.height == i64::MIN { Value::Null } else { json!(p.height) },
                })).collect::<Vec<_>>(),
            })
        }
        "panesAtPath" => Value::Array(
            adapter
                .panes_at_path(text("directory"))?
                .iter()
                .map(|p| json!({"paneId": p.pane_id, "workspaceId": p.workspace_id}))
                .collect(),
        ),
        "placePane" => {
            let placed = adapter.place_pane(PlacePaneInput {
                pane_id: text("paneId"),
                tab_id: text("tabId"),
                target_pane_id: text("targetPaneId"),
                direction: if text("direction") == "right" {
                    SplitDirection::Right
                } else {
                    SplitDirection::Down
                },
                keep: number("keep"),
                worktree_path: text("worktreePath"),
            });
            // An unknown direction word is refused by the Node adapter before anything else; Rust has two directions,
            // so the exporter only sends valid ones except "left".
            let placed = placed?;
            json!({"paneId": placed.pane_id, "workspaceId": placed.workspace_id})
        }
        "paneIdentity" => match adapter.pane_identity(text("paneId"))? {
            None => Value::Null,
            Some(identity) => {
                let mut map = Map::new();
                if let Some(v) = identity.terminal_id {
                    map.insert("terminalId".into(), json!(v));
                }
                if let Some(v) = identity.agent {
                    map.insert("agent".into(), json!(v));
                }
                if let Some(v) = identity.project {
                    map.insert("project".into(), json!(v));
                }
                Value::Object(map)
            }
        },
        "closePane" => {
            adapter.close_pane(text("paneId"))?;
            Value::Null
        }
        "removeWorktree" => {
            adapter.remove_worktree(text("workspaceId"), args["force"] == true)?;
            Value::Null
        }
        "reportMetadata" => {
            let tokens = env_of(&args["tokens"]);
            let target = if args["paneId"].is_string() {
                MetadataTarget::Pane(text("paneId"))
            } else {
                MetadataTarget::Workspace(text("workspaceId"))
            };
            adapter.report_metadata(target, &tokens)?;
            Value::Null
        }
        "renameTab" => {
            adapter.rename_tab(text("tabId"), text("label"))?;
            Value::Null
        }
        "renameWorkspace" => {
            adapter.rename_workspace(text("workspaceId"), text("label"))?;
            Value::Null
        }
        "adoptPane" => {
            adapter.adopt_pane(AdoptPaneInput {
                pane_id: text("paneId"),
                role: role_of(&args["role"]),
                agent: text("agent"),
                workspace_id: optional("workspaceId"),
                worktree_path: optional("worktreePath"),
            })?;
            Value::Null
        }
        "adoptShellPane" => {
            adapter.adopt_shell_pane(text("paneId"), optional("workspaceId"))?;
            Value::Null
        }
        other => panic!("unknown op {other}"),
    })
}

fn run_scenario(scenario: &Value) {
    let name = s(&scenario["name"]);
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path())
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let replay = Arc::new(Replay {
        root: root.clone(),
        queue: Mutex::new(VecDeque::new()),
        mismatches: Mutex::new(Vec::new()),
    });
    let clock = Arc::new(AtomicU64::new(0));
    let mut options = AdapterOptions::new(replay.clone());
    options.temp_root = Some(PathBuf::from(&root));
    options.poll_ms = Some(100);
    options.project_slug = scenario["slug"].as_str().map(str::to_string);
    let ticker = clock.clone();
    options.sleep = Some(Arc::new(move |_| {
        ticker.fetch_add(100, Ordering::SeqCst);
    }));
    let reader = clock.clone();
    options.now = Some(Arc::new(move || reader.load(Ordering::SeqCst)));
    let adapter = Adapter::new(options);
    for (index, step) in scenario["steps"].as_array().unwrap().iter().enumerate() {
        let op = s(&step["op"]);
        let label = format!("{name} step {index} ({op})");
        // The Node harness stops its clock between fake-state steps only by never advancing it; ours does the same.
        let args = {
            let mut args = denormalise(&step["args"], &root);
            // A text answer that Node typed with an unknown direction word cannot be sent to Rust's two-valued enum.
            if op == "placePane" && args["direction"] == "left" {
                args["direction"] = json!("right");
            }
            args
        };
        replay.load(step["calls"].as_array().unwrap());
        let events = Mutex::new(Vec::new());
        let result = if op == "placePane" && step["args"]["direction"] == "left" {
            // Rust cannot represent the word; the Node refusal is checked as text below.
            Err(AdapterError::InvalidArgument(
                "split direction is not acceptable".into(),
            ))
        } else {
            perform(&adapter, op, &args, &events)
        };
        let outcome = match &result {
            Ok(value) => json!({"ok": value}),
            Err(error) => json!({"error": error_json(error)}),
        };
        let outcome = normalise(&outcome, &root);
        assert!(
            replay.mismatches.lock().unwrap().is_empty(),
            "{label}: {:?}",
            replay.mismatches.lock().unwrap()
        );
        assert!(
            replay.queue.lock().unwrap().is_empty(),
            "{label}: Node made calls Rust did not: {:?}",
            replay.queue.lock().unwrap()
        );
        expect_eq(&format!("{label}: outcome"), &outcome, &step["outcome"]);
        expect_eq(
            &format!("{label}: hook and key events"),
            &json!(events.lock().unwrap().clone()),
            &step["events"],
        );
        assert_eq!(
            clock.load(Ordering::SeqCst),
            step["clock"].as_u64().unwrap(),
            "{label}: poll waits"
        );
    }
    adapter.close();
}

#[test]
fn the_adapter_makes_the_same_calls_and_answers_as_node() {
    let scenarios = load("adapter.json");
    let list = scenarios.as_array().unwrap();
    assert!(list.len() > 80, "every scenario is there");
    for scenario in list {
        run_scenario(scenario);
    }
}

#[test]
fn a_prompt_file_is_private_and_close_removes_it() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let mut options = AdapterOptions::new(Arc::new(StubRunner::new()));
    options.temp_root = Some(directory.path().to_path_buf());
    let adapter = Adapter::new(options);
    let first = adapter.write_prompt_file("# role\nbe good").unwrap();
    let second = adapter.write_prompt_file("another").unwrap();
    assert_ne!(first, second);
    let mode = std::fs::metadata(&first).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let parent = Path::new(&first).parent().unwrap().to_path_buf();
    assert_eq!(
        std::fs::metadata(&parent).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(std::fs::read_to_string(&first).unwrap(), "# role\nbe good");
    assert_eq!(
        adapter.write_prompt_file("  ").unwrap_err().message(),
        "prompt text is not acceptable"
    );
    adapter.close();
    assert!(!parent.exists());
}
