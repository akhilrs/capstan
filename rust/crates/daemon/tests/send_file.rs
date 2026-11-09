//! `send --file` and the body limit of `send`: a committed file is handed over by reference, and a long body from an
//! agent other than the PM is refused with a hint. The kernel, the ledger and git are real; there is no launcher.

mod common;

use capstan_daemon::deps::{DaemonOptions, Deps, KernelHandle, LogEntry, Logger};
use capstan_daemon::handlers::handle_frame;
use capstan_daemon::handlers::shared::{AbortSignal, MAX_SEND_BODY_BYTES};
use capstan_daemon::handlers::MAX_RESPONSE_BYTES;
use capstan_daemon::ports::ProjectGit;
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::Core;
use common::project::{build_repo, write_project, Identity};
use common::{private_tempdir, seeded_env};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
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

const NOTE: &str = "# Notes\nfirst\n";

static WORLDS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

struct World {
    _dir: common::PrivateDir,
    _wt: common::PrivateDir,
    deps: Deps,
    credentials: HashMap<String, String>,
    branch_tip: String,
    worktree: std::path::PathBuf,
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

impl World {
    /// `more` is appended to capstan.toml; `None` runs the daemon without a configuration.
    fn new(more: Option<&str>) -> World {
        let dir = private_tempdir();
        let wt = private_tempdir();
        let identity = Identity {
            project_id: "psend".into(),
            name: "Send".into(),
            credential: format!("operator-{}", "s".repeat(40)),
        };
        let state = write_project(dir.path(), &identity);
        let trust = common::kept_trust_store();
        if let Some(more) = more {
            let file = dir.path().join("capstan.toml");
            std::fs::write(&file, format!("{CONFIG}{more}")).unwrap();
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
            common::trusted_project(dir.path(), &trust);
        }
        let heads = build_repo(
            dir.path(),
            &json!({
                "files": {"a.txt": "a\n"},
                "branches": {"feat/dev-1": {"files": {"docs/note.md": NOTE}, "message": "docs: note"}},
            }),
        );
        let worktree = wt.path().join("dev-1");
        git(
            dir.path(),
            &[
                "worktree",
                "add",
                "--quiet",
                worktree.to_str().unwrap(),
                "feat/dev-1",
            ],
        );
        let database = capstan_ledger::resolve_database_path(&state).unwrap();
        capstan_ledger::open_database(&database, &capstan_ledger::OpenOptions::default()).unwrap();
        let project = identity.initial_project();
        let kernel = {
            let (state, project, root) = (state.clone(), project.clone(), dir.path().to_path_buf());
            KernelHandle::spawn(move || {
                Core::open(
                    &state,
                    &project,
                    &KernelOptions {
                        workspace_root: Some(root),
                        ..KernelOptions::default()
                    },
                    seeded_env(&format!(
                        "send-{}-{}",
                        std::process::id(),
                        WORLDS.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                    )),
                )
            })
            .expect("the kernel opens")
        };
        let sink: Logger = Arc::new(|_: &LogEntry| {});
        let mut options = DaemonOptions::new(state, project, dir.path().to_path_buf());
        options.trust_store = trust.clone();
        if more.is_some() {
            let role_config = capstan_config::load_role_config_with(dir.path(), &trust).unwrap();
            options.capstan = Some(Arc::new(role_config));
        }
        let mut deps = Deps::new(kernel, options, sink);
        deps.git = Arc::new(ProjectGit::new(dir.path()));
        capstan_daemon::deps::startup_sequence(&deps);
        let mut world = World {
            _dir: dir,
            _wt: wt,
            deps,
            credentials: HashMap::new(),
            branch_tip: heads["feat/dev-1"].clone(),
            worktree,
        };
        world
            .credentials
            .insert("operator".into(), identity.credential);
        world.populate();
        world
    }

    fn ledger(&self, credential: &str, op: &str, args: Vec<Value>) -> Value {
        let credential = credential.to_string();
        let name = op.to_string();
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

    fn populate(&mut self) {
        let owner = self.credentials["operator"].clone();
        let roles: Vec<Value> = [("pm", "PM"), ("developer", "Developer")]
            .iter()
            .map(|(name, kind)| {
                json!({"name": name, "kind": kind, "host": "claude", "configHash": "a".repeat(64)})
            })
            .collect();
        self.ledger(&owner, "syncRoleDefinitions", vec![json!(roles)]);
        for (agent, role, kind) in [
            ("pm-1", "pm", "PM"),
            ("dev-1", "developer", "Developer"),
            ("dev-2", "developer", "Developer"),
        ] {
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
            self.credentials
                .insert(agent.into(), actor["credential"].as_str().unwrap().into());
        }
        let internal = self.deps.credential().to_string();
        self.ledger(
            &internal,
            "recordAgentPane",
            vec![json!({
                "agentId": "dev-1",
                "workspaceId": "ws-1",
                "paneId": "pane-dev-1",
                "worktreePath": self.worktree.to_str().unwrap(),
                "branch": "feat/dev-1",
                "baseSha": self.branch_tip,
            })],
        );
    }

    fn call(&self, who: &str, args: &[&str]) -> Value {
        let frame =
            json!({"v": 1, "credential": self.credentials[who], "command": "send", "args": args});
        let outcome = handle_frame(
            &self.deps,
            frame.to_string().as_bytes(),
            &AbortSignal::new(),
            MAX_RESPONSE_BYTES,
        );
        serde_json::from_str(&outcome.response.expect("an answer")).expect("JSON")
    }

    fn ok(&self, who: &str, args: &[&str]) -> Value {
        let answer = self.call(who, args);
        assert_eq!(answer["ok"], true, "{args:?}: {answer}");
        answer["result"].clone()
    }

    fn refused(&self, who: &str, args: &[&str]) -> String {
        let answer = self.call(who, args);
        assert_eq!(answer["ok"], false, "{args:?}: {answer}");
        format!(
            "{}: {}",
            answer["code"],
            answer["message"].as_str().unwrap()
        )
    }

    fn pm_inbox(&self) -> Vec<String> {
        let frame =
            json!({"v": 1, "credential": self.credentials["pm-1"], "command": "inbox", "args": []});
        let outcome = handle_frame(
            &self.deps,
            frame.to_string().as_bytes(),
            &AbortSignal::new(),
            MAX_RESPONSE_BYTES,
        );
        let answer: Value = serde_json::from_str(&outcome.response.unwrap()).unwrap();
        answer["result"]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["body"].as_str().unwrap().to_string())
            .collect()
    }
}

const NEEDS_BRANCH: &str =
    "send --file needs your own branch: only an agent with a worktree can hand over a file";
const PATH_RULE: &str =
    "the file path must be relative to the repository root, without .., and at most 512 bytes";

#[test]
fn a_committed_file_goes_to_the_pm_by_reference() {
    let world = World::new(Some(""));
    let result = world.ok(
        "dev-1",
        &["@pm", "--file", "docs/note.md", "--summary", "the plan"],
    );
    let commit = world.branch_tip.clone();
    assert_eq!(
        result["file"],
        json!({"path": "docs/note.md", "branch": "feat/dev-1", "commit": commit, "size": NOTE.len()})
    );
    assert!(result["messageId"].is_string());
    assert!(result.get("uncommitted").is_none());
    let expected = format!(
        "File handed over: docs/note.md\nBranch feat/dev-1 at {commit}, {} bytes.\nRead it with: git show {commit}:docs/note.md\nSummary: the plan",
        NOTE.len()
    );
    assert_eq!(world.pm_inbox(), vec![expected]);
    let shown = Command::new("git")
        .arg("-C")
        .arg(world._dir.path())
        .args(["show", &format!("{commit}:docs/note.md")])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&shown.stdout), NOTE);
}

#[test]
fn without_a_summary_the_body_has_three_lines_and_the_user_gets_the_same() {
    let world = World::new(Some(""));
    let result = world.ok("dev-1", &["--info", "@user", "--file", "docs/note.md"]);
    assert!(result["sequence"].is_number());
    assert_eq!(result["file"]["path"], "docs/note.md");
    world.ok("dev-1", &["--info", "@pm", "--file", "docs/note.md"]);
    let bodies = world.pm_inbox();
    assert_eq!(bodies[0].lines().count(), 3);
    assert!(!bodies[0].contains("Summary:"));
}

#[test]
fn an_uncommitted_edit_sets_the_flag_and_the_warning() {
    let world = World::new(Some(""));
    std::fs::write(world.worktree.join("docs/note.md"), "edited\n").unwrap();
    let result = world.ok("dev-1", &["@pm", "--file", "docs/note.md"]);
    assert_eq!(result["uncommitted"], true);
    assert_eq!(
        result["warning"],
        "docs/note.md has uncommitted changes; the message names the committed version"
    );
    assert_eq!(result["file"]["size"], NOTE.len());
}

#[test]
fn bad_paths_summaries_and_callers_are_refused() {
    let world = World::new(Some(""));
    let rule = format!("\"invalid_request\": {PATH_RULE}");
    for path in ["../x", "a/../x", "/etc/passwd", "-x", ":x", "a\nb"] {
        assert_eq!(
            world.refused("dev-1", &["@pm", "--file", path]),
            rule,
            "{path:?}"
        );
    }
    let long = "a".repeat(513);
    assert_eq!(world.refused("dev-1", &["@pm", "--file", &long]), rule);
    let missing = format!(
        "\"invalid_request\": nope.md is not a file at feat/dev-1 ({}); commit it on your branch first",
        &world.branch_tip[..12]
    );
    assert_eq!(
        world.refused("dev-1", &["@pm", "--file", "nope.md"]),
        missing
    );
    let directory = missing.replace("nope.md", "docs");
    assert_eq!(
        world.refused("dev-1", &["@pm", "--file", "docs"]),
        directory
    );
    let summary = "s".repeat(1001);
    assert_eq!(
        world.refused(
            "dev-1",
            &["@pm", "--file", "docs/note.md", "--summary", &summary]
        ),
        "\"invalid_request\": the summary may be at most 1000 bytes"
    );
    world.ok(
        "dev-1",
        &[
            "@pm",
            "--file",
            "docs/note.md",
            "--summary",
            &"s".repeat(1000),
        ],
    );
    assert_eq!(
        world.refused("dev-1", &["@user", "--file", "docs/note.md", "--option", "x"]),
        "\"invalid_request\": send --file takes a recipient, --file <path> and an optional --summary <text>"
    );
    let needs = format!("\"invalid_request\": {NEEDS_BRANCH}");
    assert_eq!(
        world.refused("pm-1", &["dev-1", "--file", "docs/note.md"]),
        needs
    );
    assert_eq!(
        world.refused("dev-2", &["@pm", "--file", "docs/note.md"]),
        needs
    );
}

#[test]
fn the_body_limit_follows_the_config_for_agents_other_than_the_pm() {
    let world = World::new(Some("\n[messages]\nmax_body_bytes = 4096\n"));
    let hint = "\"body_too_large\": a message body may be at most 4096 bytes ([messages] max_body_bytes); write the text to a file on your branch, commit it and send cstan send --file <path> --summary <text>";
    assert_eq!(
        world.refused("dev-1", &["--info", "@pm", &"x".repeat(4097)]),
        hint
    );
    world.ok("dev-1", &["--info", "@pm", &"x".repeat(4096)]);
    world.ok("pm-1", &["--info", "dev-1", &"x".repeat(8000)]);
}

#[test]
fn without_a_config_the_old_limit_and_message_hold() {
    let world = World::new(None);
    world.ok(
        "dev-1",
        &["--info", "@pm", &"x".repeat(MAX_SEND_BODY_BYTES)],
    );
    assert_eq!(
        world.refused(
            "dev-1",
            &["--info", "@pm", &"x".repeat(MAX_SEND_BODY_BYTES + 1)]
        ),
        format!("\"body_too_large\": a message body may be at most {MAX_SEND_BODY_BYTES} bytes")
    );
}

#[test]
fn the_config_ceiling_is_the_daemons_limit() {
    assert_eq!(
        capstan_config::MAX_BODY_BYTES_LIMIT,
        MAX_SEND_BODY_BYTES as i64
    );
}

#[test]
fn the_hint_is_there_at_the_ceiling_too() {
    let world = World::new(Some("\n[messages]\nmax_body_bytes = 15360\n"));
    let answer = world.refused("dev-1", &["--info", "@pm", &"x".repeat(15361)]);
    assert!(
        answer.contains("cstan send --file <path> --summary <text>"),
        "{answer}"
    );
    world.ok("dev-1", &["--info", "@pm", &"x".repeat(15360)]);
}

#[test]
fn the_git_show_hint_is_quoted_for_a_path_with_a_space() {
    let world = World::new(Some(""));
    let dir = world.worktree.join("my docs");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("it's.md"), "x\n").unwrap();
    git(&world.worktree, &["add", "--all"]);
    git(
        &world.worktree,
        &["commit", "--quiet", "-m", "docs: spaced"],
    );
    world.ok("dev-1", &["@pm", "--file", "my docs/it's.md"]);
    let body = world.pm_inbox().remove(0);
    let line = body.lines().nth(2).unwrap();
    assert!(line.starts_with("Read it with: git show '"), "{line}");
    assert!(line.ends_with(":my docs/it'\\''s.md'"), "{line}");
}
