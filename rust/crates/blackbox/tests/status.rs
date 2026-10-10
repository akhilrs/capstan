//! Status, Nexora links, plan cancel, `status --watch` and the dashboard hand-off (test/status-active-tasks.test.ts,
//! status-feed, status-query-cost, nexora-commands, watch, dash-poller): what each caller sees in the status, what the link
//! commands record, and the watch frame an operator keeps open.

use capstan_blackbox::*;
use serde_json::{json, Value};
use std::io::Read;
use std::process::{Command, Stdio};

fn team() -> World {
    World::builder("status")
        .config(TEAM_CONFIG)
        .launching()
        .agents(&[
            agent("pm-1", "pm", "PM"),
            agent("arch-1", "architect", "Developer"),
            agent("ops-1", "operator", "Developer"),
            agent("dev-1", "developer", "Developer"),
            agent("dev-2", "developer", "Developer"),
        ])
        .build()
}

fn field(out: &Out, key: &str) -> String {
    out.stdout
        .lines()
        .find_map(|l| l.strip_prefix(&format!("{key}: ")))
        .unwrap_or_else(|| panic!("no {key} in {}", out.stdout))
        .trim()
        .to_string()
}

fn body(ids: &[&str]) -> String {
    json!({
        "summary": "split",
        "packages": ids.iter().map(|id| json!({
            "id": id, "title": format!("title of {id}"), "owns": [format!("src/{id}/")],
            "interfaces": [], "acceptance": ["works"], "estimate_hours": 1,
        })).collect::<Vec<_>>(),
    })
    .to_string()
}

fn approved_plan(w: &World, ids: &[&str]) -> String {
    let plan = field(
        &w.as_agent("pm-1", &["plan", "open", "normal", "the plan"])
            .ok(),
        "planId",
    );
    w.as_agent("arch-1", &["plan", "submit", &plan, &body(ids)])
        .ok();
    plan
}

fn status_as(w: &World, who: Option<&str>) -> Value {
    match who {
        Some(agent) => w.wire_as(agent, "status", &[])["result"].clone(),
        None => w.wire_op("status", &[])["result"].clone(),
    }
}

#[test]
fn link_commands_are_for_the_pm_and_the_operator_and_refuse_bad_input() {
    let w = team();
    let plan = approved_plan(&w, &["wp1"]);
    for who in ["dev-1", "arch-1", "ops-1"] {
        let out = w.as_agent(who, &["link", "plan", &plan, "PM-47"]);
        assert_eq!(out.code, 4, "{who}: {}", out.text());
    }
    // The external id must look like PM-47.
    for bad in ["47", "PM-", "PM-4 7", "PM47", "-47"] {
        let out = w.as_agent("pm-1", &["link", "plan", &plan, bad]);
        assert_eq!(out.code, 3, "{bad}: {}", out.text());
        assert!(
            out.stderr.contains("must look like PM-47"),
            "{}",
            out.stderr
        );
    }
    // An unknown plan; a different Nexora id for a linked plan; the same id again is fine.
    assert_ne!(
        w.as_agent("pm-1", &["link", "plan", "plan-99", "PM-47"])
            .code,
        0
    );
    w.as_agent("pm-1", &["link", "plan", &plan, "PM-47"]).ok();
    assert_ne!(
        w.as_agent("pm-1", &["link", "plan", &plan, "PM-48"]).code,
        0
    );
    w.op(&["link", "plan", &plan, "PM-47"]).ok();
    // A package link, and a bind needs a linked requirement.
    w.as_agent(
        "pm-1",
        &[
            "link",
            "package",
            &format!("{plan}/wp1"),
            "PM-50",
            "in_progress",
        ],
    )
    .ok();
    let unlinked = w.as_agent("pm-1", &["link", "bind", "req-1", "dev-1"]);
    assert_eq!(unlinked.code, 4, "{}", unlinked.text());
    assert!(
        unlinked.stderr.contains("link_refused") && unlinked.stderr.contains("run link first"),
        "{}",
        unlinked.stderr
    );
}

#[test]
fn plan_show_prints_the_links_and_only_the_pm_status_carries_the_drift() {
    let w = team();
    let plan = approved_plan(&w, &["wp1"]);
    w.as_agent("pm-1", &["link", "plan", &plan, "PM-47"]).ok();
    w.as_agent(
        "pm-1",
        &[
            "link",
            "package",
            &format!("{plan}/wp1"),
            "PM-50",
            "in_progress",
        ],
    )
    .ok();
    let shown = w.as_agent("pm-1", &["plan", "show", &plan]).ok();
    assert!(
        shown.stdout.contains("PM-47") && shown.stdout.contains("PM-50"),
        "{}",
        shown.stdout
    );
    let pm = status_as(&w, Some("pm-1"));
    assert!(
        pm.get("nexoraDrift").is_some(),
        "{:?}",
        pm.as_object().unwrap().keys().collect::<Vec<_>>()
    );
    for other in ["dev-1", "arch-1"] {
        assert!(
            status_as(&w, Some(other)).get("nexoraDrift").is_none(),
            "{other} has no drift section"
        );
    }
    assert!(status_as(&w, None).get("nexoraDrift").is_none());
}

#[test]
fn plan_cancel_is_for_the_operator_and_tells_the_pm_and_the_architect() {
    let w = team();
    let plan = approved_plan(&w, &["wp1", "wp2"]);
    w.as_agent("pm-1", &["plan", "assign", &plan, "wp1", "dev-1"])
        .ok();
    w.as_agent("pm-1", &["plan", "assign", &plan, "wp2", "dev-2"])
        .ok();
    for who in ["pm-1", "arch-1", "dev-1"] {
        let out = w.as_agent(who, &["plan", "cancel", &plan]);
        assert_eq!(out.code, 4, "{who}: {}", out.text());
        assert!(
            out.stderr.contains("only the operator may cancel a plan"),
            "{}",
            out.stderr
        );
    }
    // One package leaves the rest alone.
    let one = w.op(&["plan", "cancel", &plan, "wp1"]).ok();
    assert_eq!(field(&one, "packageId"), "wp1");
    let shown = w.op(&["plan", "show", &plan]).ok().stdout;
    assert!(
        shown.contains("state: approved"),
        "the plan stands: {shown}"
    );
    let all = w.op(&["plan", "cancel", &plan]).ok();
    assert!(
        all.stdout.contains("pm-1"),
        "the PM is told: {}",
        all.stdout
    );
    let pm_inbox = w.as_agent("pm-1", &["inbox"]).ok().stdout;
    assert!(
        pm_inbox.contains("cancel") || pm_inbox.contains("Cancel"),
        "{pm_inbox}"
    );
    // The assignee hears of its cancelled package.
    let dev = w.as_agent("dev-2", &["inbox"]).ok().stdout;
    assert!(dev.to_lowercase().contains("cancel"), "{dev}");
}

#[test]
fn only_the_operator_gets_active_tasks_and_panes_and_cancelled_plans_are_absent() {
    let w = team();
    let plan = approved_plan(&w, &["wp1", "wp2"]);
    w.as_agent("pm-1", &["plan", "assign", &plan, "wp1", "dev-1"])
        .ok();
    let operator = status_as(&w, None);
    let tasks = &operator["activeTasks"];
    assert!(tasks.is_object() || tasks.is_array(), "{tasks}");
    let text = tasks.to_string();
    assert!(
        text.contains("title of wp1") && text.contains("title of wp2") && text.contains("dev-1"),
        "{text}"
    );
    assert!(operator.get("panes").is_some());
    for other in [Some("pm-1"), Some("dev-1"), Some("arch-1")] {
        let status = status_as(&w, other);
        assert!(
            status.get("activeTasks").is_none() && status.get("panes").is_none(),
            "{other:?}"
        );
    }
    w.op(&["plan", "cancel", &plan]).ok();
    assert!(
        !status_as(&w, None)["activeTasks"]
            .to_string()
            .contains("title of wp1"),
        "a cancelled plan is not an active task"
    );
}

#[test]
fn the_operator_status_carries_pipeline_totals_and_pending_proposals_only_with_the_operator_on() {
    let w = team();
    let status = status_as(&w, None);
    assert!(status["pipelineCounts"].is_object(), "{status}");
    assert!(
        status.get("integrations").is_some()
            && status.get("reviews").is_some()
            && status.get("awaitingConfirm").is_some()
    );
    assert_eq!(
        status["promptRelay"]["enabled"], true,
        "the prompt relay section is there when configured: {}",
        status["promptRelay"]
    );
    let proposal = w.as_agent("ops-1", &["op", "propose", "ls", "why"]).ok();
    let id = field(&proposal, "proposalId");
    let pending = &status_as(&w, None)["pendingProposals"];
    assert!(pending.to_string().contains(&id), "{pending}");
    // Without [operator] the status has the counts and no pendingProposals.
    let plain = World::standard("status-plain");
    let status = status_as(&plain, None);
    assert!(status.get("pendingProposals").is_none() && status["pipelineCounts"].is_object());
    assert!(
        status.get("promptRelay").is_none() || status["promptRelay"].is_null(),
        "no prompt relay section without one"
    );
}

fn watch_frame(w: &World, extra: &[&str]) -> String {
    let mut child = Command::new(cstan_binary())
        .args(["status", "--watch", "--interval", "1"])
        .args(extra)
        .current_dir(&w.project)
        .env_clear()
        .envs(w.environment())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut seen = String::new();
    let mut buffer = [0u8; 512];
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !seen.contains("\n---\n") && std::time::Instant::now() < deadline {
        let n = stdout.read(&mut buffer).unwrap();
        if n == 0 {
            break;
        }
        seen.push_str(&String::from_utf8_lossy(&buffer[..n]));
    }
    let _ = child.kill();
    let _ = child.wait();
    seen
}

#[test]
fn the_watch_frame_lists_the_agents_and_the_unresolved_messages_without_control_characters() {
    let w = World::standard("watch");
    let first = watch_frame(&w, &[]);
    assert_eq!(
        first,
        "agents: dev-1 (Developer, active), dev-2 (Developer, active), pm-1 (PM, active), rev-1 (Verifier, active), sup-1 (Supervisor, active)\nmessages: none unresolved\n---\n",
        "the first frame lists every agent and rings no bell"
    );
    let bare = World::builder("watch-empty").build();
    assert_eq!(
        watch_frame(&bare, &[]),
        "agents: none\nmessages: none unresolved\n---\n"
    );
    // A message with control characters is refused at the door, so no frame can carry one.
    let refused = w.as_agent(
        "pm-1",
        &["send", "dev-1", "line one\u{1b}[31m red \u{7}bell"],
    );
    assert_eq!(refused.code, 3, "{}", refused.text());
    w.as_agent("pm-1", &["send", "dev-1", "a plain message"])
        .ok();
    let frame = watch_frame(&w, &[]);
    assert!(
        frame.contains("-> dev-1 [queued] notified: no") && !frame.contains("none unresolved"),
        "{frame}"
    );
    assert!(
        !frame.contains('\u{1b}') && !frame.contains('\u{7}'),
        "no control character is printed: {frame:?}"
    );
    // The interval is checked.
    let bad = w.op(&["status", "--watch", "--interval", "0"]);
    assert_eq!(bad.code, 3, "{}", bad.text());
    assert!(
        bad.stderr
            .contains("--interval must be an integer from 1 to 60"),
        "{}",
        bad.stderr
    );
}

#[test]
fn the_watch_ends_when_the_controller_stops_answering() {
    let mut w = World::standard("watchend");
    let mut child = Command::new(cstan_binary())
        .args(["status", "--watch", "--interval", "1"])
        .current_dir(&w.project)
        .env_clear()
        .envs(w.environment())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1500));
    w.stop();
    let ended = until("the watch to end", 30, || child.try_wait().unwrap());
    assert!(!ended.success() || ended.code().is_some(), "{ended:?}");
}

const PTY_RUNNER: &str = r#"
import os, pty, select, sys, time
seconds = float(sys.argv[1])
pid, fd = pty.fork()
if pid == 0:
    os.execvpe(sys.argv[2], sys.argv[2:], os.environ)
out = b""
end = time.time() + seconds
sent = False
while time.time() < end:
    ready, _, _ = select.select([fd], [], [], 0.2)
    if ready:
        try:
            data = os.read(fd, 65536)
        except OSError:
            break
        if not data:
            break
        out += data
    elif not sent and out:
        os.write(fd, b"q")
        sent = True
try:
    os.kill(pid, 15)
except OSError:
    pass
try:
    _, status = os.waitpid(pid, 0)
except OSError:
    status = 0
sys.stdout.buffer.write(out)
sys.stdout.flush()
sys.exit(os.waitstatus_to_exitcode(status) if os.waitstatus_to_exitcode(status) >= 0 else 0)
"#;

fn python() -> Option<&'static str> {
    ["/usr/bin/python3", "/usr/local/bin/python3"]
        .into_iter()
        .find(|p| std::path::Path::new(p).is_file())
}

/// Runs `cstan dash` under a terminal (the PTY runner needs python3, which only this test helper uses, not cstan).
fn dash_on_a_terminal(w: &World, dash_bin: &str, seconds: u64) -> (i32, String) {
    let python = python().expect("python3");
    let out = Command::new(python)
        .arg("-c")
        .arg(PTY_RUNNER)
        .arg(seconds.to_string())
        .arg(cstan_binary())
        .arg("dash")
        .arg("--no-color")
        .current_dir(&w.project)
        .env_clear()
        .envs(w.environment())
        .env("CSTAN_DASH_BIN", dash_bin)
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

#[test]
fn dash_needs_a_terminal_and_the_cstan_dash_binary_and_with_both_draws_a_frame() {
    let w = World::standard("dash");
    // Without a terminal the command points at `status`.
    let piped = w.op(&["dash"]);
    assert_eq!(piped.code, 2, "{}", piped.text());
    assert!(
        piped.stderr.contains("dash needs an interactive terminal"),
        "{}",
        piped.stderr
    );
    let Some(_) = python() else {
        eprintln!(
            "SKIPPED LOUDLY: dash on a terminal: python3 is not available for the terminal runner"
        );
        return;
    };
    // On a terminal with no cstan-dash named or found: the install hint.
    let (code, shown) = dash_on_a_terminal(&w, "/nonexistent/cstan-dash", 10);
    assert_eq!(code, 2, "{shown}");
    assert!(shown.contains("cstan-dash was not found"), "{shown}");
    // With the real binary (it builds in dash/, apart from the workspace): one frame.
    let Some(dash) = dash_binary() else {
        eprintln!("SKIPPED LOUDLY: cstan-dash frame: set CSTAN_DASH_BIN to a built cstan-dash (cargo build --release --manifest-path dash/Cargo.toml)");
        return;
    };
    let (_, frame) = dash_on_a_terminal(&w, dash.to_str().unwrap(), 10);
    assert!(
        frame.contains("pm-1") && frame.contains("dev-1"),
        "the frame lists the team: {frame}"
    );
}
