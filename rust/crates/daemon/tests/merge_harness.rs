//! The decide-merge test harness: the per-daemon merge state, the seams and their source scans, and `MergeProject`.

mod common;

use capstan_daemon::handlers::decide_merge_seams::{install_seams, seams, DecideMergeSeams};
use capstan_daemon::handlers::shared::CommandState;
use common::merge::MergeProject;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::Duration;

// ------------------------------------------------------------------------------------------------ the project

#[test]
fn a_new_project_has_a_merged_integration_with_a_passed_review_and_a_passed_verify_at_its_head() {
    let project = MergeProject::new();
    let integration = project.integration();
    assert_eq!(integration["state"], "merged", "{integration}");
    let review = project.latest_review();
    assert_eq!(review["state"], "passed", "{review}");
    assert_eq!(review["commitSha"], integration["headSha"], "{review}");
    let runs = project.verify_runs();
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert_eq!(runs[0]["state"], "passed");
    assert_eq!(runs[0]["commitSha"], integration["headSha"]);
    assert_eq!(
        runs[0]["commandsSha"],
        project.current_commands_sha().as_str()
    );
    assert_eq!(
        project.rev(&project.branch),
        integration["headSha"].as_str().unwrap()
    );
    // The developer's agent is gone, the PM's is active.
    let agent = |id: &'static str| {
        project
            .deps
            .kernel
            .run(move |core| core.agent_record(id))
            .unwrap()
    };
    let pm = agent("pm-1");
    assert_eq!(pm["state"], "active", "{pm}");
    let dev = agent("dev-1");
    assert_ne!(dev["state"], "active", "{dev}");
    assert!(project.launcher_calls().is_empty());
}

#[test]
fn the_project_repository_has_its_own_identity() {
    let project = MergeProject::new();
    let root = project.dir.path();
    let value = |key: &str| {
        let out = std::process::Command::new("git")
            .args(["config", "--local", key])
            .current_dir(root)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    assert!(!value("user.name").is_empty());
    assert!(!value("user.email").is_empty());
}

#[test]
fn each_helper_changes_only_its_own_fact() {
    let mut project = MergeProject::new();
    let before = project.integration();
    project.set_verify_commands(&["false"]);
    assert_ne!(
        project.current_commands_sha(),
        project.verify_runs()[0]["commandsSha"].as_str().unwrap()
    );
    assert_eq!(project.latest_review()["state"], "passed");
    assert_eq!(project.integration(), before);
    let config = capstan_config::load_role_config(project.dir.path()).unwrap();
    assert!(config.remote.merge);

    project.set_review_findings();
    assert_eq!(project.latest_review()["state"], "findings");
    assert_eq!(project.integration(), before);
    assert_eq!(project.verify_runs().len(), 1);

    let tip = project.move_branch_tip();
    assert_ne!(Some(tip.as_str()), before["headSha"].as_str());
    assert_eq!(project.rev(&project.branch), tip);
    assert_eq!(project.integration(), before);
    assert_eq!(project.verify_runs().len(), 1);
}

#[test]
fn set_remote_merge_false_is_read_by_the_start_up_loader() {
    let mut project = MergeProject::new();
    assert!(
        capstan_config::load_role_config(project.dir.path())
            .unwrap()
            .remote
            .merge
    );
    project.set_remote_merge(false);
    let config = capstan_config::load_role_config(project.dir.path()).unwrap();
    assert!(!config.remote.merge);
    assert!(config.verify.is_some());
    project.set_remote_merge(true);
    assert!(
        capstan_config::load_role_config(project.dir.path())
            .unwrap()
            .remote
            .merge
    );
}

#[test]
fn decide_merge_builds_the_decide_argv_and_reaches_the_route() {
    let project = MergeProject::new();
    let argv = project.decide_merge_argv("n1");
    assert_eq!(argv[0], format!("integration:{}", project.integration_id));
    assert_eq!(
        [
            argv[1].as_str(),
            argv[3].as_str(),
            argv[4].as_str(),
            argv[5].as_str()
        ],
        ["--expect", "--nonce", "n1", "merge"]
    );
    assert_eq!(argv[2].len(), 12);
    // Until the route exists the daemon answers an unknown command; either way it answers.
    let answer = project.decide_merge("n1");
    assert!(answer["ok"].is_boolean(), "{answer}");
}

#[test]
fn the_socket_helper_answers_a_ping() {
    let project = MergeProject::new();
    let answer = project.socket_call("operator", "ping", &[]);
    assert_eq!(answer["ok"], true, "{answer}");
}

// ------------------------------------------------------------------------------------------------ the seams

#[test]
fn the_default_seams_are_all_off() {
    let seams = DecideMergeSeams::default();
    assert!(seams.git_seams.is_none() && seams.merge_seams.is_none());
    assert!(seams.after_a.is_none() && seams.hold_b.is_none() && seams.hold_after_marker.is_none());
    assert!(seams.block_pm_message.is_none() && seams.lookup_depth.is_none());
    assert!(
        !(seams.crash_in_b
            || seams.crash_after_b
            || seams.fail_record
            || seams.fail_settle
            || seams.panic_in_b)
    );
}

#[test]
fn seams_are_per_daemon() {
    let (one, other) = (MergeProject::new(), MergeProject::new());
    assert!(seams(&one.deps).is_none() && seams(&other.deps).is_none());
    install_seams(
        &one.deps,
        DecideMergeSeams {
            lookup_depth: Some(7),
            ..DecideMergeSeams::default()
        },
    );
    assert_eq!(seams(&one.deps).unwrap().lookup_depth, Some(7));
    assert!(seams(&other.deps).is_none());
}

#[test]
fn a_hold_blocks_until_a_send_and_until_the_sender_is_dropped() {
    use capstan_daemon::handlers::decide_merge_seams::wait_hold;
    for drop_sender in [false, true] {
        let (sender, receiver) = mpsc::channel();
        let hold = Some(Mutex::new(receiver));
        let (done_tx, done_rx) = mpsc::channel();
        let waiter = std::thread::spawn(move || {
            wait_hold(&hold);
            done_tx.send(()).unwrap();
        });
        assert!(done_rx.recv_timeout(Duration::from_millis(150)).is_err());
        if drop_sender {
            drop(sender);
        } else {
            sender.send(()).unwrap();
        }
        done_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("released");
        waiter.join().unwrap();
    }
}

#[test]
fn a_default_command_state_has_an_unlocked_merge_lock_and_no_seams() {
    let state = CommandState::default();
    assert!(state.remote_merge.try_lock().is_ok());
    assert!(state.merge_seams.lock().unwrap().is_none());
}

// ------------------------------------------------------------------------------------------------ source scans

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The part of `line` before a `//` comment.
fn code_of(line: &str) -> &str {
    line.find("//").map_or(line, |at| &line[..at])
}

/// The 1-based lines of `text` with a call of `name(` in code: comments and the definition do not count.
fn call_lines(text: &str, name: &str) -> Vec<usize> {
    let needle = format!("{name}(");
    text.lines()
        .enumerate()
        .filter(|(_, line)| {
            let code = code_of(line);
            code.contains(&needle) && !code.contains(&format!("fn {needle}"))
        })
        .map(|(index, _)| index + 1)
        .collect()
}

#[test]
fn the_call_matcher_finds_calls_and_not_comments_or_the_definition() {
    assert_eq!(
        call_lines("install_seams(&deps, s);", "install_seams"),
        vec![1]
    );
    assert_eq!(
        call_lines("x\n    install_seams(d, s)", "install_seams"),
        vec![2]
    );
    assert!(call_lines("// install_seams(d, s)", "install_seams").is_empty());
    assert!(call_lines("/// call install_seams(d, s)", "install_seams").is_empty());
    assert!(call_lines("pub fn install_seams(deps: &Deps) {", "install_seams").is_empty());
    assert_eq!(
        call_lines("let r = GitRepo::new(root); // x", "GitRepo::new"),
        vec![1]
    );
    assert!(call_lines("let r = 1; // GitRepo::new(root)", "GitRepo::new").is_empty());
}

fn src_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    rust_files(&common::crate_dir().join("src"), &mut files);
    assert!(!files.is_empty());
    files
}

/// The hits of `name(` in the daemon's sources, outside the files that may hold them.
fn hits(name: &str, allowed: &[&str]) -> Vec<String> {
    let mut found = Vec::new();
    for file in src_files() {
        if allowed.iter().any(|a| file.ends_with(a)) {
            continue;
        }
        let text = std::fs::read_to_string(&file).unwrap();
        for line in call_lines(&text, name) {
            found.push(format!("{}:{line}", file.display()));
        }
    }
    found
}

#[test]
fn only_the_seams_file_has_an_install_call() {
    let found = hits("install_seams", &["handlers/decide_merge_seams.rs"]);
    assert!(
        found.is_empty(),
        "install_seams called in the sources: {found:?}"
    );
}

#[test]
fn the_merge_handler_builds_its_repository_through_seamed_repo() {
    // merge-core adds decide_merge.rs; until then the scan has nothing to look at.
    let handler = common::crate_dir().join("src/handlers/decide_merge.rs");
    if !handler.exists() {
        return;
    }
    let found = hits("GitRepo::new", &[]);
    let mine: Vec<_> = found
        .iter()
        .filter(|h| h.contains("handlers/decide_merge.rs:"))
        .collect();
    assert!(
        mine.is_empty(),
        "decide_merge.rs builds a GitRepo itself: {mine:?}"
    );
}
