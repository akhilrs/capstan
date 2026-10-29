//! `cstan verify` end to end: the real binary (which embeds the daemon) on a scratch git project with a `[verify]`
//! section. The gate is two portable shell commands. The test seeds the ledger with an agent, a report and an approved plan
//! package the way the daemon would have recorded them, then asks for a run, reads it back, and checks the three places a
//! run shows: `cstan verify show`, the verify section of `cstan status` and the gate of `cstan plan show --brief`.
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use capstan_kernel::helpers::execute;
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::{Core, SystemEnv};
use serde_json::{json, Value};

const OPERATOR: &str = "operator-0123456789abcdef0123456789abcdef0123456789";
const REPORTED_AT: &str = "2026-01-02T00:00:00.000Z";
const ASSIGNED_AT: &str = "2026-01-01T00:00:00.000Z";

struct Project {
    base: PathBuf,
    root: PathBuf,
    commit: String,
}

impl Drop for Project {
    fn drop(&mut self) {
        // Stop the daemon this test started before its directory goes.
        let _ = Command::new(env!("CARGO_BIN_EXE_cstan"))
            .arg("stop")
            .current_dir(&self.root)
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("HOME", "/nonexistent")
            .stdin(Stdio::null())
            .output();
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn private(path: &Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git runs");
    assert!(output.status.success(), "git {args:?}: {output:?}");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn step(core: &Core, op: &str, args: Value) -> Value {
    let context = json!({
        "credential": OPERATOR,
        "requestId": format!("req-{op}-{}", core.state_version().unwrap()),
        "idempotencyKey": format!("idem-{op}-{}", core.state_version().unwrap()),
        "expectedVersion": core.state_version().unwrap(),
        "inputRevision": core.input_revision().unwrap(),
    });
    let mut all = vec![context];
    all.extend(args.as_array().cloned().unwrap());
    core.dispatch(op, &all)
        .unwrap_or_else(|e| panic!("setup {op}: {e}"))
}

fn project(label: &str, verify: &str) -> Project {
    let base = capstan_testkit::short_temp_root()
        .join(format!("capstan-verify-e2e-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let root = base.join("proj");
    let scratch = base.join("scratch");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&scratch).unwrap();
    git(&root, &["init", "--quiet", "--initial-branch=main"]);
    std::fs::write(root.join("a.txt"), "a\n").unwrap();
    std::fs::write(root.join(".git/info/exclude"), ".capstan/\n").unwrap();
    git(&root, &["add", "--all"]);
    git(&root, &["commit", "--quiet", "-m", "chore: initial commit"]);
    std::fs::write(root.join("one.txt"), "one\n").unwrap();
    git(&root, &["add", "--all"]);
    git(&root, &["commit", "--quiet", "-m", "feat: one"]);
    let commit = git(&root, &["rev-parse", "HEAD"]);
    let capstan = root.join(".capstan");
    let state = capstan.join("state");
    std::fs::create_dir_all(&state).unwrap();
    private(&capstan, 0o700);
    private(&state, 0o700);
    let config = json!({
        "schemaVersion": 1, "projectId": "pverifye2e", "name": "Verify",
        "stateDirectory": state.to_string_lossy(),
        "maxSlices": 4, "maxRunMs": 3_600_000, "maxDispatches": 16,
    });
    for (name, text) in [
        ("project.json", format!("{config:#}\n")),
        ("operator.key", format!("{OPERATOR}\n")),
    ] {
        std::fs::write(capstan.join(name), text).unwrap();
        private(&capstan.join(name), 0o600);
    }
    let toml = format!(
        "schema_version = 1\n[nexora]\ntrack = \"never\"\n[hosts.claude]\nkind = \"claude\"\n\
         [architect]\nenabled = true\n[roles.pm]\nkind = \"PM\"\nhost = \"claude\"\n[roles.architect]\nkind = \"Developer\"\nhost = \"claude\"\n[roles.developer]\nkind = \"Developer\"\nhost = \"claude\"\n\
         [verify]\nscratch_dir = \"{}\"\n{verify}\n",
        scratch.display()
    );
    std::fs::write(root.join("capstan.toml"), toml).unwrap();
    private(&root.join("capstan.toml"), 0o600);
    let project = Project { base, root, commit };
    seed(&project, &state);
    project
}

/// An agent, its accepted report at the commit and an approved plan whose package it holds, written before the daemon opens.
fn seed(project: &Project, state: &Path) {
    let initial = capstan_kernel::types::InitialProject {
        project_id: "pverifye2e".into(),
        name: "Verify".into(),
        owner_credential: OPERATOR.into(),
        initial_inputs: capstan_daemon::run::placeholder_inputs(),
    };
    let core = Core::open(
        state,
        &initial,
        &KernelOptions {
            workspace_root: Some(project.root.clone()),
            ..KernelOptions::default()
        },
        Box::new(SystemEnv),
    )
    .expect("the ledger opens");
    step(
        &core,
        "syncRoleDefinitions",
        json!([[
            {"name": "developer", "kind": "Developer", "host": "claude", "configHash": "a".repeat(64)},
            {"name": "pm", "kind": "PM", "host": "claude", "configHash": "b".repeat(64)},
        ]]),
    );
    let seat = "seat-dev-1";
    step(
        &core,
        "createSeat",
        json!([{"seatId": seat, "name": seat, "role": "Developer"}]),
    );
    let actor = step(
        &core,
        "createActor",
        json!([{"displayName": "dev-1", "role": "Developer", "seatId": seat}]),
    );
    step(
        &core,
        "registerAgent",
        json!([{"agentId": "dev-1", "roleName": "developer", "seatId": seat, "actorId": actor["actorId"]}]),
    );
    core.close();
    let database = capstan_ledger::resolve_database_path(state).unwrap();
    let db =
        capstan_ledger::open_database(&database, &capstan_ledger::OpenOptions::default()).unwrap();
    let actor_id = actor["actorId"].as_str().unwrap().to_string();
    execute(
        &db,
        "INSERT INTO agent_reports(project_id, report_id, sequence, agent_id, generation, actor_id, commit_sha, branch, summary, state, evidence_json, created_at)
         VALUES (?, 'report-1', 1, 'dev-1', 1, ?, ?, 'feat/dev-1', 'did the work', 'accepted', '{}', ?)",
        ["pverifye2e", actor_id.as_str(), project.commit.as_str(), REPORTED_AT],
    )
    .unwrap();
    execute(
        &db,
        "INSERT INTO plans(project_id, plan_id, sequence, title, tier, state, requested_by, current_revision, approved_revision, created_at, updated_at)
         VALUES (?, 'plan-1', 1, 'Gate plan', 'normal', 'draft', 'operator', 0, NULL, ?, ?)",
        ["pverifye2e", ASSIGNED_AT, ASSIGNED_AT],
    )
    .unwrap();
    let body = json!({"title": "Gate plan", "packages": [{"id": "p1", "type": "feature", "estimate_hours": 2, "depends_on": []}]}).to_string();
    execute(
        &db,
        "INSERT INTO plan_revisions(project_id, plan_id, revision, base_sha, body_json, body_sha, author_agent_id, author_actor_id, created_at)
         VALUES (?, 'plan-1', 1, ?, ?, ?, 'dev-1', ?, ?)",
        ["pverifye2e", project.commit.as_str(), body.as_str(), "c".repeat(64).as_str(), actor_id.as_str(), ASSIGNED_AT],
    )
    .unwrap();
    execute(
        &db,
        "UPDATE plans SET state = 'approved', current_revision = 1, approved_revision = 1 WHERE project_id = ? AND plan_id = 'plan-1'",
        ["pverifye2e"],
    )
    .unwrap();
    execute(
        &db,
        "INSERT INTO plan_packages(project_id, plan_id, package_id, assignee_agent_id, assigned_at) VALUES (?, 'plan-1', 'p1', 'dev-1', ?)",
        ["pverifye2e", ASSIGNED_AT],
    )
    .unwrap();
}

impl Project {
    fn cstan(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_cstan"))
            .args(args)
            .current_dir(&self.root)
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("HOME", "/nonexistent")
            .stdin(Stdio::null())
            .output()
            .expect("cstan runs")
    }

    fn out(&self, args: &[&str]) -> String {
        let output = self.cstan(args);
        assert!(
            output.status.success(),
            "cstan {args:?} failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// Waits for a run to leave queued and running; returns the output of `verify show`.
    fn finished(&self, id: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            let shown = self.out(&["verify", "show", id]);
            let first = shown.lines().next().unwrap_or("");
            if !first.ends_with(": queued") && !first.ends_with(": running") {
                return shown;
            }
            assert!(Instant::now() < deadline, "the run did not end: {shown}");
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

#[test]
fn a_gate_run_is_queued_run_recorded_and_shown_in_verify_status_and_plan_show() {
    let project = project("pass", "commands = [\"echo lint-ok\", \"echo tests-ok\"]\n");
    let queued = project.out(&["verify", "report-1"]);
    let line = queued.lines().next().unwrap();
    assert!(line.starts_with("verify run "), "{queued}");
    assert!(
        line.contains(&format!("queued at {}", &project.commit[..7])),
        "{queued}"
    );
    let id = line.split_whitespace().nth(2).unwrap().to_string();
    let shown = project.finished(&id);
    assert!(
        shown.lines().next().unwrap().ends_with(": passed"),
        "{shown}"
    );
    assert!(shown.contains("tests-ok"), "{shown}");
    // The record, as JSON.
    let json: Value =
        serde_json::from_str(&project.out(&["verify", "show", &id, "--json"])).unwrap();
    assert_eq!(json["state"], "passed");
    assert_eq!(json["exitCode"], 0);
    assert_eq!(json["commitSha"], project.commit);
    assert_eq!(json["subjectId"], "report-1");
    // A repeat after the run has ended starts a new one; the list has both.
    let listed = project.out(&["verify", "list", "report-1"]);
    assert!(
        listed.contains(&id) && listed.contains("passed"),
        "{listed}"
    );
    // The status of the operator shows the verify section.
    let status_json: Value = serde_json::from_str(&project.out(&["status", "--json"])).unwrap();
    assert_eq!(status_json["verify"]["recent"][0]["runId"], id.as_str());
    let status = project.out(&["status"]);
    assert!(status.contains("verify recent:"), "{status}");
    assert!(
        status.contains(&format!("{id} passed report report-1")),
        "{status}"
    );
    // The plan row shows the gate of the package's report.
    let plan = project.out(&["plan", "show", "plan-1", "--brief"]);
    let row = plan
        .lines()
        .find(|l| l.trim_start().starts_with("p1 "))
        .expect("a package row");
    assert!(row.contains("gate passed"), "{plan}");
    // No scratch worktree is left in the project.
    assert_eq!(
        git(&project.root, &["worktree", "list", "--porcelain"])
            .lines()
            .filter(|l| l.starts_with("worktree "))
            .count(),
        1
    );
}

#[test]
fn a_failing_gate_shows_its_step_and_a_project_without_verify_refuses_the_command() {
    let project = project(
        "fail",
        "commands = [\"echo fine\", \"echo broken-here >&2; exit 5\"]\n",
    );
    let queued = project.out(&["verify", "report-1"]);
    let id = queued.split_whitespace().nth(2).unwrap().to_string();
    let shown = project.finished(&id);
    assert!(
        shown.lines().next().unwrap().ends_with(": failed"),
        "{shown}"
    );
    assert!(shown.contains("broken-here"), "{shown}");
    let plan = project.out(&["plan", "show", "plan-1", "--brief"]);
    assert!(plan.contains("gate failed step 2"), "{plan}");
    let status = project.out(&["status"]);
    assert!(
        status.contains("verify recent:") && status.contains("failed"),
        "{status}"
    );
    // Without [verify] the section is absent and the command is refused.
    let bare = std::fs::read_to_string(project.root.join("capstan.toml")).unwrap();
    let cut = bare.find("[verify]").unwrap();
    project.out(&["stop"]);
    std::fs::write(project.root.join("capstan.toml"), &bare[..cut]).unwrap();
    let refused = project.cstan(&["verify", "report-1"]);
    assert!(!refused.status.success());
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&refused.stdout),
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(text.contains("verify_not_configured"), "{text}");
    let status = project.out(&["status"]);
    assert!(
        !status.contains("verify recent:") && !status.contains("verify active:"),
        "{status}"
    );
}
