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
        }
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
        json!({"path": "docs/capstan/kb.jsonl", "maxEntries": 7})
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
        json!({"path": "docs/capstan/kb.jsonl", "maxEntries": 7})
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
