//! The `kb` route through `handlers::handle_frame` like a connection, on a scratch git repository with a `[kb]` table and a
//! real ledger: the read forms, the briefs and the check the `add` and `update` commands run.

mod common;

use capstan_daemon::deps::{DaemonOptions, Deps, KernelHandle, Logger};
use capstan_daemon::handlers::handle_frame;
use capstan_daemon::handlers::shared::AbortSignal;
use capstan_daemon::handlers::MAX_RESPONSE_BYTES;
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::Core;
use common::project::{write_project, Identity};
use common::{private_tempdir, seeded_env};
use serde_json::{json, Value};
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

const CONFIG: &str = r#"schema_version = 1

[hosts.claude]
kind = "claude"

[roles.pm]
kind = "PM"
host = "claude"

[roles.developer]
kind = "Developer"
host = "claude"
"#;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

struct World {
    _dir: common::PrivateDir,
    deps: Deps,
    credential: String,
    agents: std::collections::HashMap<String, String>,
}

fn entry(id: &str, kind: &str, title: &str, body: &str, path: Option<&str>, head: &str) -> String {
    let mut e = json!({"id": id, "kind": kind, "title": title, "body": body});
    if let Some(path) = path {
        e["path"] = json!(path);
    }
    e["verified_commit"] = json!(head);
    e.to_string()
}

impl World {
    /// `kb` is the text of the `[kb]` section, or `None` for none.
    fn new(kb: Option<&str>) -> World {
        let dir = private_tempdir();
        let identity = Identity {
            project_id: "pkbroute".into(),
            name: "Kb".into(),
            credential: format!("operator-{}", "k".repeat(40)),
        };
        let state = write_project(dir.path(), &identity);
        let root = dir.path();
        git(root, &["init", "-q", "-b", "main"]);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/util.rs"), "fn a() {}\n").unwrap();
        git(root, &["add", "src"]);
        git(root, &["commit", "-q", "-m", "chore: start"]);
        let head = git(root, &["rev-parse", "HEAD"]);
        std::fs::create_dir_all(root.join("docs/capstan")).unwrap();
        let lines = [
            entry(
                "fmt-bytes",
                "helper",
                "Format a byte count",
                "Turns 1536 into 1.5 KiB. Ünï body ",
                Some("src/util.rs"),
                &head,
            ),
            entry(
                "retry-policy",
                "convention",
                "Retry policy",
                "Retries with backoff.",
                None,
                &head,
            ),
        ];
        std::fs::write(
            root.join("docs/capstan/kb.jsonl"),
            format!("{}\n{}\n", lines[0], lines[1]),
        )
        .unwrap();
        git(root, &["add", "docs"]);
        git(root, &["commit", "-q", "-m", "docs: kb"]);
        let file = root.join("capstan.toml");
        let kb = kb.map(|k| format!("\n[kb]\n{k}\n")).unwrap_or_default();
        std::fs::write(&file, format!("{CONFIG}{kb}")).unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let trust = common::kept_trust_store();
        common::trusted_project(root, &trust);
        let database = capstan_ledger::resolve_database_path(&state).unwrap();
        capstan_ledger::open_database(&database, &capstan_ledger::OpenOptions::default()).unwrap();
        let project = identity.initial_project();
        let kernel = {
            let (state, project, root) = (state.clone(), project.clone(), root.to_path_buf());
            KernelHandle::spawn(move || {
                Core::open(
                    &state,
                    &project,
                    &KernelOptions {
                        workspace_root: Some(root),
                        ..KernelOptions::default()
                    },
                    seeded_env("kb-route"),
                )
            })
            .expect("the kernel opens")
        };
        let sink: Logger = Arc::new(|_| {});
        let mut options = DaemonOptions::new(state, project, root.to_path_buf());
        options.trust_store = trust.clone();
        options.capstan = Some(Arc::new(
            capstan_config::load_role_config_with(root, &trust).unwrap(),
        ));
        let deps = Deps::new(kernel, options, sink);
        capstan_daemon::deps::startup_sequence(&deps);
        World {
            _dir: dir,
            deps,
            credential: identity.credential,
            agents: Default::default(),
        }
    }

    fn ledger(&self, credential: &str, op: &str, args: Vec<Value>) -> Value {
        let (credential, name) = (credential.to_string(), op.to_string());
        self.deps
            .kernel
            .run(move |core| {
                let context = capstan_daemon::deps::new_context(core, &credential)?;
                let mut all = vec![serde_json::to_value(context).unwrap()];
                all.extend(args);
                core.dispatch(&name, &all)
            })
            .unwrap_or_else(|e| panic!("{op}: {e}"))
    }

    /// Registers the PM `pm-1` and the developer `dev-1`.
    fn with_agents(mut self) -> World {
        let owner = self.credential.clone();
        let roles = json!([
            {"name": "pm", "kind": "PM", "host": "claude", "configHash": "a".repeat(64)},
            {"name": "developer", "kind": "Developer", "host": "claude", "configHash": "a".repeat(64)},
        ]);
        self.ledger(&owner, "syncRoleDefinitions", vec![roles]);
        for (agent, role, kind) in [("pm-1", "pm", "PM"), ("dev-1", "developer", "Developer")] {
            let seat = format!("seat-{agent}");
            self.ledger(
                &owner,
                "createSeat",
                vec![json!({"seatId": seat, "name": seat, "role": kind})],
            );
            let actor = self.ledger(
                &owner,
                "createActor",
                vec![json!({"displayName": agent, "role": kind, "seatId": seat})],
            );
            self.ledger(
                &owner,
                "registerAgent",
                vec![json!({"agentId": agent, "roleName": role, "seatId": seat, "actorId": actor["actorId"]})],
            );
            self.agents
                .insert(agent.into(), actor["credential"].as_str().unwrap().into());
        }
        self
    }

    fn pm_messages(&self) -> Vec<Value> {
        let messages = self
            .deps
            .kernel
            .run(|core| core.messages_for("pm-1"))
            .expect("the PM's messages");
        messages.as_array().cloned().unwrap_or_default()
    }

    /// One call as `who` (`operator` or an agent id): the whole answer.
    fn call_as(&self, who: &str, args: &[&str]) -> Value {
        let credential = self.agents.get(who).unwrap_or(&self.credential);
        let frame = json!({"v": 1, "credential": credential, "command": "kb", "args": args});
        let outcome = handle_frame(
            &self.deps,
            frame.to_string().as_bytes(),
            &AbortSignal::new(),
            MAX_RESPONSE_BYTES,
        );
        serde_json::from_str(&outcome.response.expect("an answer")).expect("JSON")
    }

    fn as_ok(&self, who: &str, args: &[&str]) -> Value {
        let answer = self.call_as(who, args);
        assert_eq!(answer["ok"], true, "{args:?}: {answer}");
        answer["result"].clone()
    }

    fn call(&self, args: &[&str]) -> Value {
        let frame = json!({"v": 1, "credential": self.credential, "command": "kb", "args": args});
        let outcome = handle_frame(
            &self.deps,
            frame.to_string().as_bytes(),
            &AbortSignal::new(),
            MAX_RESPONSE_BYTES,
        );
        serde_json::from_str(&outcome.response.expect("an answer")).expect("JSON")
    }

    fn result(&self, args: &[&str]) -> Value {
        let answer = self.call(args);
        assert_eq!(answer["ok"], true, "{args:?}: {answer}");
        answer["result"].clone()
    }

    fn refused(&self, args: &[&str]) -> String {
        let result = self.result(args);
        result["refused"]
            .as_str()
            .unwrap_or_else(|| panic!("{args:?}: {result}"))
            .to_string()
    }
}

#[test]
fn search_for_uses_and_status_answer_with_hits_and_status() {
    let w = World::new(Some(""));
    let found = w.result(&["search", "bytes"]);
    let hits = found["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["id"], "fmt-bytes");
    assert_eq!(hits[0]["path"], "src/util.rs");
    assert_eq!(hits[0]["state"], "ok");
    assert!(hits[0]["body"].as_str().unwrap().starts_with("Turns 1536"));
    assert_eq!(
        w.result(&["search", "bytes", "retry", "--limit", "1"])["hits"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        w.result(&["search", "zzzzqqq"])["hits"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(w.result(&["for", "src"])["hits"][0]["id"], "fmt-bytes");
    assert_eq!(
        w.result(&["for", "docs"])["hits"].as_array().unwrap().len(),
        0
    );
    assert_eq!(
        w.result(&["uses", "fmt-bytes"])["hits"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    let status = &w.result(&["status"])["status"];
    assert_eq!(status["entries"], 2);
    assert_eq!(status["state"], "ok");
    assert_eq!(status["searches30d"], 3);
}

#[test]
fn bad_arguments_are_invalid_requests() {
    let w = World::new(Some(""));
    for args in [
        &["bogus"][..],
        &["search"],
        &["search", "x", "--limit", "0"],
        &["uses"],
        &["status", "x"],
        &["build", "x"],
    ] {
        let answer = w.call(args);
        assert_eq!(answer["ok"], false, "{args:?}: {answer}");
        assert_eq!(answer["code"], "invalid_request", "{args:?}");
    }
}

#[test]
fn without_a_kb_table_or_when_disabled_every_form_is_refused() {
    for kb in [None, Some("enabled = false")] {
        let w = World::new(kb);
        for args in [
            &["search", "x"][..],
            &["for", "src"],
            &["uses", "a"],
            &["status"],
            &["build"],
            &["refresh"],
            &["check", "path"],
        ] {
            assert_eq!(
                w.refused(args),
                "kb: not configured; add a [kb] table to capstan.toml",
                "{args:?}"
            );
        }
    }
}

#[test]
fn briefs_name_the_role_and_the_file() {
    let w = World::new(Some("role = \"developer\""));
    let build = w.result(&["build"]);
    assert!(build["brief"]
        .as_str()
        .unwrap()
        .contains("Build the knowledge base at docs/capstan/kb.jsonl"));
    assert_eq!(build["role"], "developer");
    let refresh = w.result(&["refresh"]);
    assert!(refresh["brief"]
        .as_str()
        .unwrap()
        .contains("(none are stale or missing)"));
    let design = w.result(&["build", "--design"]);
    assert!(design["brief"].as_str().unwrap().contains("design record"));
    assert_eq!(design["role"], "designer");
}

#[test]
fn check_applies_the_field_rules_the_head_ids_and_the_similar_entries() {
    let w = World::new(Some("max_entries = 7"));
    let head = git(w._dir.path(), &["rev-parse", "HEAD"]);
    assert_eq!(
        w.result(&["check", "path"]),
        json!({"path": "docs/capstan/kb.jsonl", "maxEntries": 7, "storage": "repo", "location": "docs/capstan/kb.jsonl"})
    );
    let fresh = entry(
        "dates",
        "helper",
        "Parse a calendar date",
        "ISO only.",
        None,
        &head,
    );
    assert_eq!(
        w.result(&["check", "add", &fresh]),
        json!({"path": "docs/capstan/kb.jsonl", "maxEntries": 7, "storage": "repo", "location": "docs/capstan/kb.jsonl"})
    );
    let bad = entry("Bad Id", "helper", "t", "b", None, &head);
    assert!(w
        .refused(&["check", "add", &bad])
        .starts_with("kb entry: id must match"));
    let exists = entry("fmt-bytes", "helper", "Other thing", "b", None, &head);
    assert_eq!(
        w.refused(&["check", "add", &exists]),
        "kb: id fmt-bytes exists; use cstan kb update fmt-bytes"
    );
    let similar = entry(
        "fmt-size",
        "helper",
        "Format a byte count",
        "b",
        None,
        &head,
    );
    assert_eq!(
        w.refused(&["check", "add", &similar]),
        "kb: similar entries exist: fmt-bytes; reuse or update one, or add --force"
    );
    w.result(&["check", "add", &similar, "--force"]);
    // update has no id or similar check, but the field rules hold
    w.result(&["check", "update", &exists]);
    assert!(w
        .refused(&["check", "update", &bad])
        .starts_with("kb entry: id must match"));
    let answer = w.call(&["check", "add"]);
    assert_eq!(answer["code"], "invalid_request");
}

const LOCAL: &str = "storage = \"local\"\npm_note_minutes = 30";

fn local_entry(w: &World, id: &str, title: &str) -> String {
    let head = git(w._dir.path(), &["rev-parse", "HEAD"]);
    entry(id, "convention", title, "A body.", None, &head)
}

#[test]
fn repo_storage_refuses_write_and_import_and_check_path_names_the_storage() {
    let w = World::new(Some("")).with_agents();
    let head = git(w._dir.path(), &["rev-parse", "HEAD"]);
    let path = w.result(&["check", "path"]);
    assert_eq!(path["storage"], "repo");
    assert_eq!(path["location"], "docs/capstan/kb.jsonl");
    let e = entry("dates", "helper", "Parse a date", "b", None, &head);
    assert_eq!(
        w.refused(&["write", "add", &e]),
        "kb: [kb] storage is repo; edit the entries file with cstan kb add in your working tree and commit it"
    );
    assert!(w
        .refused(&["import", "0123456789abcdef"])
        .starts_with("kb: [kb] storage is repo;"));
    assert!(w
        .refused(&["import-part", "0123456789abcdef", "x\n"])
        .starts_with("kb: [kb] storage is repo;"));
    assert_eq!(
        w.refused(&["log"]),
        "kb: [kb] storage is repo; edit the entries file with cstan kb add in your working tree and commit it"
    );
    // get and export read the committed file
    let got = w.result(&["get", "fmt-bytes"]);
    assert!(got["entry"].as_str().unwrap().contains("fmt-bytes"));
    assert!(w.result(&["export"])["text"]
        .as_str()
        .unwrap()
        .contains("retry-policy"));
}

#[test]
fn local_write_get_log_export_and_refusals() {
    let w = World::new(Some(LOCAL)).with_agents();
    let path = w.result(&["check", "path"]);
    assert_eq!(path["storage"], "local");
    assert!(path["location"]
        .as_str()
        .unwrap()
        .ends_with("kb/entries.jsonl"));
    assert_eq!(w.result(&["export"])["text"], "");
    let e = local_entry(&w, "dates", "Parse a calendar date");
    let saved = w.as_ok("operator", &["write", "add", &e]);
    assert_eq!(saved["verb"], "add");
    assert_eq!(saved["id"], "dates");
    assert_eq!(saved["saved"], path["location"]);
    assert!(w
        .refused(&["write", "add", &e])
        .starts_with("kb: id dates exists;"));
    let same = local_entry(&w, "dates-two", "Parse a calendar date");
    assert!(w
        .refused(&["write", "add", &same])
        .starts_with("kb: similar entries exist: dates;"));
    w.as_ok("operator", &["write", "add", &same, "--force"]);
    let upd = local_entry(&w, "dates", "Parse a calendar date, ISO");
    assert_eq!(w.result(&["write", "update", &upd])["verb"], "update");
    let missing = local_entry(&w, "nope", "Nothing");
    assert!(w
        .refused(&["write", "update", &missing])
        .starts_with("kb: no entry nope in "));
    assert!(w.result(&["get", "dates"])["entry"]
        .as_str()
        .unwrap()
        .contains("ISO"));
    assert!(w
        .refused(&["get", "zzz"])
        .starts_with("kb: no entry zzz in "));
    assert_eq!(
        w.result(&["write", "remove", "dates-two"])["verb"],
        "remove"
    );
    assert!(w
        .refused(&["write", "remove", "dates-two"])
        .starts_with("kb: no entry"));
    let log = w.result(&["log"]);
    let verbs: Vec<&str> = log["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["verb"].as_str().unwrap())
        .collect();
    assert_eq!(verbs, ["remove", "update", "add", "add"]);
    assert_eq!(
        w.result(&["log", "--limit", "1"])["changes"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(w.call(&["log", "--limit", "0"])["code"], "invalid_request");
    assert_eq!(
        w.result(&["export"])["text"]
            .as_str()
            .unwrap()
            .lines()
            .count(),
        1
    );
}

const TOKEN: &str = "0123456789abcdef";

fn parts(w: &World) -> usize {
    std::fs::read_dir(w.deps.options.state_directory.join("kb"))
        .map(|d| {
            d.flatten()
                .filter(|e| e.file_name().to_string_lossy().ends_with(".part"))
                .count()
        })
        .unwrap_or(0)
}

#[test]
fn import_is_for_the_pm_or_the_operator_and_staged_in_parts() {
    let w = World::new(Some(LOCAL)).with_agents();
    let chunk = format!("{}\nbad\n", local_entry(&w, "imp", "Imported entry"));
    for args in [
        vec!["import-part", TOKEN, &chunk[..]],
        vec!["import", TOKEN],
    ] {
        assert_eq!(
            w.call_as("dev-1", &args)["result"]["refused"],
            "kb: import is for the PM or the operator"
        );
    }
    assert_eq!(parts(&w), 0);
    // two parts, one write
    let (a, b) = chunk.split_at(chunk.find('\n').unwrap() + 1);
    w.as_ok("pm-1", &["import-part", TOKEN, a]);
    w.as_ok("pm-1", &["import-part", TOKEN, b]);
    assert_eq!(parts(&w), 1);
    let done = w.as_ok("pm-1", &["import", TOKEN]);
    assert_eq!(done["added"], 1);
    assert_eq!(done["invalid"][0]["line"], 2);
    assert_eq!(parts(&w), 0);
    w.as_ok("operator", &["import-part", TOKEN, &chunk]);
    let again = w.as_ok("operator", &["import", TOKEN, "--replace"]);
    assert_eq!(again["replaced"], 1);
    let log = w.result(&["log"]);
    assert_eq!(log["changes"].as_array().unwrap().len(), 2);
    assert_eq!(log["changes"][1]["verb"], "import");
    // a missing part and a bad token
    assert!(w
        .refused(&["import", TOKEN])
        .starts_with("kb: no staged import"));
    assert_eq!(w.call(&["import", "XYZ"])["code"], "invalid_request");
    // imports never notify the PM
    assert!(w.pm_messages().is_empty());
}

#[test]
fn a_staged_import_over_the_limits_writes_nothing_and_leaves_no_part() {
    let w = World::new(Some("storage = \"local\"\nmax_entries = 2")).with_agents();
    w.as_ok(
        "operator",
        &["write", "add", &local_entry(&w, "one", "First rule")],
    );
    let before = w.result(&["export"])["text"].clone();
    let lines: String = ["a", "b"]
        .iter()
        .map(|id| format!("{}\n", local_entry(&w, id, &format!("Title {id} {id}{id}"))))
        .collect();
    w.as_ok("operator", &["import-part", TOKEN, &lines]);
    assert!(w
        .refused(&["import", TOKEN])
        .contains("over the limit of 2"));
    assert_eq!(w.result(&["export"])["text"], before);
    assert_eq!(parts(&w), 0);
    // the staged size cap: max_entries * max_entry_bytes
    let cap = (2 * w
        .deps
        .options
        .capstan
        .as_ref()
        .unwrap()
        .kb
        .as_ref()
        .unwrap()
        .max_entry_bytes) as usize;
    w.as_ok("operator", &["import-part", TOKEN, "x"]);
    assert!(w
        .refused(&["import-part", TOKEN, &"y".repeat(cap)])
        .contains("over the limit"));
    assert_eq!(parts(&w), 0);
}

#[test]
fn a_stale_expect_is_refused_in_update_and_remove() {
    let w = World::new(Some(LOCAL)).with_agents();
    w.as_ok(
        "operator",
        &["write", "add", &local_entry(&w, "dates", "Parse dates")],
    );
    let got = w.result(&["get", "dates"]);
    let sha = got["sha"].as_str().unwrap().to_string();
    assert_eq!(sha.len(), 64);
    let upd = local_entry(&w, "dates", "Parse dates, ISO");
    w.as_ok("operator", &["write", "update", &upd, "--expect", &sha]);
    let msg = "kb: dates changed since it was read; run the command again.";
    let again = local_entry(&w, "dates", "Parse dates, again");
    assert_eq!(
        w.refused(&["write", "update", &again, "--expect", &sha]),
        msg
    );
    assert_eq!(
        w.refused(&["write", "remove", "dates", "--expect", &sha]),
        msg
    );
    let fresh = w.result(&["get", "dates"])["sha"]
        .as_str()
        .unwrap()
        .to_string();
    w.as_ok(
        "operator",
        &["write", "remove", "dates", "--expect", &fresh],
    );
}

#[test]
fn a_failed_note_keeps_the_window_open() {
    let w = World::new(Some(LOCAL)).with_agents();
    capstan_daemon::handlers::kb::fail_next_note(&w.deps.options.state_directory);
    w.as_ok(
        "dev-1",
        &["write", "add", &local_entry(&w, "first", "First entry")],
    );
    assert!(w.pm_messages().is_empty());
    w.as_ok(
        "dev-1",
        &[
            "write",
            "add",
            &local_entry(&w, "second", "Second thing here"),
        ],
    );
    assert_eq!(w.pm_messages().len(), 1);
}

#[test]
fn a_developer_write_notes_the_pm_once_per_window() {
    let w = World::new(Some(LOCAL)).with_agents();
    let first = local_entry(&w, "first", "First entry");
    w.as_ok("dev-1", &["write", "add", &first]);
    let messages = w.pm_messages();
    assert_eq!(messages.len(), 1, "{messages:?}");
    let body = messages[0]["body"].as_str().unwrap();
    assert!(
        body.starts_with("kb: dev-1 added first (First entry)"),
        "{body}"
    );
    let second = local_entry(&w, "second", "Second thing here");
    w.as_ok("dev-1", &["write", "add", &second]);
    assert_eq!(w.pm_messages().len(), 1);
    // PM and operator writes never note
    let third = local_entry(&w, "third", "Third other item");
    w.as_ok("pm-1", &["write", "add", &third]);
    w.as_ok("operator", &["write", "remove", "third"]);
    assert_eq!(w.pm_messages().len(), 1);
    // after the window
    let state = w.deps.options.state_directory.clone();
    capstan_daemon::handlers::kb::age_last_note(&state, std::time::Duration::from_secs(31 * 60));
    w.as_ok("dev-1", &["write", "remove", "second"]);
    let messages = w.pm_messages();
    assert_eq!(messages.len(), 2);
    assert!(messages[1]["body"]
        .as_str()
        .unwrap()
        .starts_with("kb: dev-1 removed second ("));
}

#[test]
fn a_zero_note_window_never_notes() {
    let w = World::new(Some("storage = \"local\"\npm_note_minutes = 0")).with_agents();
    let e = local_entry(&w, "quiet", "Quiet entry");
    w.as_ok("dev-1", &["write", "add", &e]);
    assert!(w.pm_messages().is_empty());
    assert_eq!(w.result(&["log"])["changes"][0]["agent"], "dev-1");
}
