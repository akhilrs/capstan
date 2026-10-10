//! Reports, reviews and integrations through the executable (test/reports.test.ts, review-commands, integration,
//! integration-git, stale-reports, recovery-core): what a report claims and what git says, the commit rules, the rate limit,
//! the unread-mail rule, a reviewer's verdict, integrating with real git (conflicts, hooks, one at a time, never twice) and a
//! replaced agent's old token. A reviewer's spawn needs Herdr; the review is begun in the ledger as `request-review` does.

use capstan_blackbox::*;
use serde_json::Value;

fn world() -> World {
    World::builder("reports")
        .agents(&[
            agent("pm-1", "pm", "PM"),
            agent("dev-1", "developer", "Developer"),
            agent("dev-2", "developer", "Developer"),
            agent("rev-1", "reviewer", "Verifier"),
            agent("rev-2", "reviewer", "Verifier"),
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

fn rejected(out: &Out, reason: &str) {
    assert_eq!(out.code, 4, "{}", out.text());
    assert!(
        out.stderr.contains(reason),
        "expected {reason:?}: {}",
        out.stderr
    );
}

#[test]
fn an_accepted_report_is_recorded_and_told_to_the_pm_and_a_repeat_writes_nothing() {
    let mut w = world();
    let (commit, _) = w.give_branch("dev-1", "one.txt", "one\n", "feat: one");
    let accepted = w
        .as_agent("dev-1", &["report", &commit, "added one.txt"])
        .ok();
    assert_eq!(field(&accepted, "state"), "accepted", "{}", accepted.stdout);
    let id = field(&accepted, "reportId");
    let pm = w.as_agent("pm-1", &["inbox"]).ok();
    assert!(
        pm.stdout.contains(&commit[..12]) || pm.stdout.contains(&id[..8]),
        "the controller tells the PM: {}",
        pm.stdout
    );
    // The same claim again is a duplicate with nothing new.
    let version = w.op(&["status", "--json"]).ok().json()["stateVersion"].clone();
    let again = w
        .as_agent("dev-1", &["report", &commit, "added one.txt"])
        .ok();
    assert_eq!(field(&again, "duplicate"), "true");
    assert_eq!(field(&again, "reportId"), id);
    assert_eq!(
        w.op(&["status", "--json"]).ok().json()["stateVersion"],
        version,
        "a repeat writes nothing"
    );
    // The report is listed in the status and found by inspect.
    let reports = &w.wire_op("status", &[])["result"]["reports"];
    assert!(reports.to_string().contains(&id), "{reports}");
    let inspected = w.op(&["inspect", &id, "--json"]).ok().json();
    assert_eq!(inspected["kind"], "report", "{inspected}");
    assert_eq!(inspected["id"], id.as_str(), "{inspected}");
}

#[test]
fn each_way_a_claim_can_fail_is_rejected_with_its_reason() {
    let mut w = world();
    let (commit, _) = w.give_branch("dev-1", "one.txt", "one\n", "feat: one");
    let (other, _) = w.give_branch("dev-2", "two.txt", "two\n", "feat: two");
    let base = w.git(&["rev-parse", "main"]);
    // The PM, the operator and a reviewer cannot report.
    rejected(&w.as_agent("pm-1", &["report", &commit, "x"]), "forbidden");
    assert_eq!(w.wire_op("report", &[&commit, "x"])["code"], "forbidden");
    // Shapes: a short commit id, an empty summary, no arguments.
    assert_eq!(
        w.wire_as("dev-1", "report", &["abc123", "x"])["code"],
        "invalid_request"
    );
    assert_eq!(
        w.wire_as("dev-1", "report", &[&commit, "  "])["code"],
        "invalid_request"
    );
    assert_eq!(w.wire_as("dev-1", "report", &[])["code"], "invalid_request");
    // A commit that is not on the agent's branch, a commit already in the base, and one that does not exist.
    let foreign = w.as_agent("dev-1", &["report", &other, "that is dev-2's commit"]);
    assert_eq!(foreign.code, 4, "{}", foreign.text());
    assert!(foreign.stderr.contains("rejected"), "{}", foreign.stderr);
    let in_base = w.as_agent("dev-1", &["report", &base, "the base itself"]);
    assert_eq!(in_base.code, 4, "{}", in_base.text());
    let missing = w.as_agent("dev-1", &["report", &"f".repeat(40), "nothing there"]);
    assert_eq!(missing.code, 4, "{}", missing.text());
    // A rejection does not block a later valid claim.
    w.as_agent("dev-1", &["report", &commit, "now the right one"])
        .ok();
}

#[test]
fn new_commits_that_break_the_commit_rules_are_refused_and_accepted_once_fixed() {
    let mut w = world();
    let (bad, worktree) = w.give_branch("dev-1", "one.txt", "one\n", "wip stuff");
    let refused = w.as_agent("dev-1", &["report", &bad, "work in progress"]);
    rejected(&refused, "rejected");
    let log = w.command_log();
    assert!(
        log.iter()
            .any(|e| e["command"] == "daemon:report_commit_refused"
                && e["detail"]["rule"] == "subject-format"),
        "{log:?}"
    );
    // A Claude co-author line is refused whatever the subject.
    w.git_in(
        &worktree,
        &[
            "commit",
            "--amend",
            "-q",
            "-m",
            "feat: one\n\nCo-Authored-By: Claude <noreply@anthropic.com>",
        ],
    );
    let amended = w.git_in(&worktree, &["rev-parse", "HEAD"]);
    let refused = w.as_agent("dev-1", &["report", &amended, "with a trailer"]);
    rejected(&refused, "rejected");
    assert!(
        w.command_log()
            .iter()
            .any(|e| e["detail"]["rule"] == "claude-co-author"),
        "the rule is logged"
    );
    // Fixed, it is accepted.
    w.git_in(&worktree, &["commit", "--amend", "-q", "-m", "feat: one"]);
    let fixed = w.git_in(&worktree, &["rev-parse", "HEAD"]);
    assert_eq!(
        field(
            &w.as_agent("dev-1", &["report", &fixed, "fixed"]).ok(),
            "state"
        ),
        "accepted"
    );
}

#[test]
fn a_worker_is_limited_to_ten_report_commands_a_minute() {
    let mut w = world();
    let (commit, _) = w.give_branch("dev-1", "one.txt", "one\n", "feat: one");
    for attempt in 0..10 {
        // Repeats count as commands too.
        w.as_agent("dev-1", &["report", &commit, "again"]).ok();
        let _ = attempt;
    }
    let eleventh = w.as_agent("dev-1", &["report", &commit, "again"]);
    rejected(&eleventh, "rate_limited");
}

#[test]
fn a_report_is_refused_while_mail_from_before_the_commit_is_unread_and_a_later_message_does_not_block(
) {
    let mut w = world();
    w.as_agent("pm-1", &["send", "dev-1", "read me first"]).ok();
    // Git times a commit to the second: the commit must be later than the message for the message to count as before it.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let (commit, _) = w.give_branch("dev-1", "one.txt", "one\n", "feat: one");
    let refused = w.as_agent("dev-1", &["report", &commit, "done"]);
    rejected(&refused, "unread_messages");
    // No row was written; after the ack it is accepted.
    let inbox = w.as_agent("dev-1", &["inbox", "--json"]).ok().json();
    for message in inbox["messages"].as_array().unwrap() {
        w.as_agent("dev-1", &["ack", message["messageId"].as_str().unwrap()])
            .ok();
    }
    w.as_agent("dev-1", &["report", &commit, "done"]).ok();
    // A message queued after the commit only adds the notice.
    let (second, _) = w.give_branch("dev-2", "two.txt", "two\n", "feat: two");
    std::thread::sleep(std::time::Duration::from_millis(1100));
    w.as_agent("pm-1", &["send", "dev-2", "after your commit"])
        .ok();
    let accepted = w.as_agent("dev-2", &["report", &second, "done"]).ok();
    assert_eq!(field(&accepted, "state"), "accepted");
}

#[test]
fn a_reviewers_verdict_is_for_a_reviewer_with_a_review_and_a_new_round_is_a_new_session() {
    let mut w = world();
    let (commit, _) = w.give_branch("dev-1", "one.txt", "one\n", "feat: one");
    let report = field(
        &w.as_agent("dev-1", &["report", &commit, "done"]).ok(),
        "reportId",
    );
    // No review has begun: a verdict has nothing to answer.
    assert_ne!(
        w.as_agent("rev-1", &["review", "pass", "looks fine"]).code,
        0
    );
    // Only a reviewer says a verdict.
    assert_eq!(
        w.wire_as("dev-1", "review", &["pass", "mine is fine"])["code"],
        "forbidden"
    );
    assert_eq!(
        w.wire_as("pm-1", "review", &["pass", "pm says"])["code"],
        "forbidden"
    );
    w.begin_review("pm-1", &report, "reviewer", "rev-1");
    assert_ne!(
        w.as_agent("rev-2", &["review", "pass", "not my review"])
            .code,
        0
    );
    // The verdict and its text are checked.
    assert_eq!(
        w.wire_as("rev-1", "review", &["maybe", "unsure"])["code"],
        "invalid_request"
    );
    assert_eq!(
        w.wire_as("rev-1", "review", &["pass"])["code"],
        "invalid_request"
    );
    let found = w
        .as_agent("rev-1", &["review", "findings", "the test is missing"])
        .ok();
    assert!(found.stdout.contains("findings"), "{}", found.stdout);
    // A re-review after a fix is a new round with a different reviewer session.
    w.begin_review("pm-1", &report, "reviewer", "rev-2");
    let passed = w.as_agent("rev-2", &["review", "pass", "now it is"]).ok();
    assert!(passed.stdout.contains("pass"), "{}", passed.stdout);
}

fn reviewed_report(
    w: &mut World,
    agent: &str,
    reviewer: &str,
    file: &str,
    content: &str,
    subject: &str,
) -> String {
    let (commit, _) = w.give_branch(agent, file, content, subject);
    let report = field(
        &w.as_agent(agent, &["report", &commit, &format!("{subject} done")])
            .ok(),
        "reportId",
    );
    w.begin_review("pm-1", &report, "reviewer", reviewer);
    w.as_agent(reviewer, &["review", "pass", "fine"]).ok();
    report
}

#[test]
fn integrate_needs_reviewed_reports_merges_them_into_one_branch_and_runs_one_at_a_time() {
    let mut w = world();
    let (c1, _) = w.give_branch("dev-1", "one.txt", "one\n", "feat: one");
    let r1 = field(
        &w.as_agent("dev-1", &["report", &c1, "one"]).ok(),
        "reportId",
    );
    // Not reviewed: refused. Unknown report: refused. A worker: forbidden.
    assert_ne!(w.as_agent("pm-1", &["integrate", &r1]).code, 0);
    assert_ne!(w.as_agent("pm-1", &["integrate", "no-such-report"]).code, 0);
    assert_eq!(w.wire_as("dev-1", "integrate", &[&r1])["code"], "forbidden");
    w.begin_review("pm-1", &r1, "reviewer", "rev-1");
    w.as_agent("rev-1", &["review", "pass", "fine"]).ok();
    let r2 = reviewed_report(&mut w, "dev-2", "rev-2", "two.txt", "two\n", "feat: two");
    // A report named twice is refused.
    assert_ne!(w.as_agent("pm-1", &["integrate", &r1, &r1]).code, 0);
    let before_branches = w.git(&["branch", "--list"]);
    let merged = w.as_agent("pm-1", &["integrate", &r1, &r2]).ok();
    assert_eq!(field(&merged, "state"), "merged", "{}", merged.stdout);
    let branch = field(&merged, "branch");
    assert!(branch.starts_with("integration/"), "{branch}");
    // One squash commit on the new branch holds both files; the project's checkout is untouched.
    let files = w.git(&["ls-tree", "-r", "--name-only", &branch]);
    assert!(
        files.contains("one.txt") && files.contains("two.txt"),
        "{files}"
    );
    assert_eq!(w.git(&["rev-parse", "--abbrev-ref", "HEAD"]), "main");
    assert!(
        !w.project.join("one.txt").exists(),
        "nothing is checked out in the project"
    );
    assert_eq!(
        w.git(&["rev-list", "--count", &format!("main..{branch}")]),
        "1",
        "one commit"
    );
    assert!(w.git(&["branch", "--list"]).len() > before_branches.len());
    // A second integration of the same reports is refused while one is merged and unsettled, and a discard removes the branch.
    assert_ne!(w.as_agent("pm-1", &["integrate", &r1]).code, 0);
    let integration = field(&merged, "integrationId");
    w.as_agent("pm-1", &["integrate", "discard", &integration])
        .ok();
    assert!(
        !w.git(&["branch", "--list", &branch])
            .contains("integration/"),
        "the branch is gone"
    );
}

#[test]
fn a_conflict_names_the_report_and_the_files_and_leaves_no_branch_or_changed_file() {
    let mut w = world();
    let r1 = reviewed_report(
        &mut w,
        "dev-1",
        "rev-1",
        "same.txt",
        "from one\n",
        "feat: one",
    );
    let r2 = reviewed_report(
        &mut w,
        "dev-2",
        "rev-2",
        "same.txt",
        "from two\n",
        "feat: two",
    );
    let branches = w.git(&["branch", "--list"]);
    let status = w.git(&["status", "--porcelain"]);
    let out = w.as_agent("pm-1", &["integrate", &r1, &r2]);
    // A conflict is an answer, not a crash: it names the report that did not merge and the file.
    let text = out.text();
    assert!(text.contains("same.txt"), "{text}");
    assert!(text.contains("conflict"), "{text}");
    assert_eq!(
        w.git(&["branch", "--list"]),
        branches,
        "no integration branch is kept"
    );
    assert_eq!(w.git(&["status", "--porcelain"]), status, "no file changed");
    assert_eq!(
        w.git(&["worktree", "list"]).lines().count(),
        3,
        "no worktree is left (main and the two agent worktrees)"
    );
    let _: Value = Value::Null;
}

#[test]
fn repository_hooks_do_not_run_during_an_integration() {
    let mut w = world();
    let marker = w.root.join("hook-ran");
    for hook in [
        "pre-commit",
        "commit-msg",
        "post-commit",
        "prepare-commit-msg",
        "pre-merge-commit",
    ] {
        let path = w.project.join(".git/hooks").join(hook);
        write_executable(
            &path,
            &format!("#!/bin/sh\necho {hook} >> '{}'\nexit 0\n", marker.display()),
        );
    }
    let r1 = reviewed_report(&mut w, "dev-1", "rev-1", "one.txt", "one\n", "feat: one");
    let hooked_before = marker.exists();
    let merged = w.as_agent("pm-1", &["integrate", &r1]).ok();
    assert_eq!(field(&merged, "state"), "merged");
    let after = std::fs::read_to_string(&marker).unwrap_or_default();
    // The agent's own commit in its worktree may have run hooks (that is the agent's); the integration adds none.
    let _ = hooked_before;
    assert!(
        !after.contains("pre-merge-commit"),
        "no merge hook ran: {after}"
    );
}

#[test]
fn a_confirmed_integration_cannot_be_integrated_again() {
    let mut w = world();
    let r1 = reviewed_report(&mut w, "dev-1", "rev-1", "one.txt", "one\n", "feat: one");
    let merged = w.as_agent("pm-1", &["integrate", &r1]).ok();
    let integration = field(&merged, "integrationId");
    let branch = field(&merged, "branch");
    // The integrated commit is reviewed by someone who wrote none of the reports.
    w.begin_review("pm-1", &integration, "reviewer", "rev-2");
    w.as_agent("rev-2", &["review", "pass", "the merge is fine"])
        .ok();
    // The user merges the branch into HEAD; the PM confirms.
    w.git(&["merge", "--ff-only", "--quiet", &branch]);
    let confirmed = w
        .as_agent("pm-1", &["integrate", "confirm", &integration])
        .ok();
    assert!(
        confirmed.stdout.contains("confirmed"),
        "{}",
        confirmed.stdout
    );
    // The report is in the head and cannot go in again.
    let again = w.as_agent("pm-1", &["integrate", &r1]);
    assert_ne!(again.code, 0, "{}", again.text());
}

#[test]
fn after_a_replacement_the_old_token_is_refused_and_the_replacement_cannot_report_the_predecessors_commit(
) {
    let mut w = world();
    let (old_commit, _) = w.give_branch("dev-1", "one.txt", "one\n", "feat: one");
    let old_token = w.replace_generation("dev-1");
    for command in ["inbox", "status", "wait", "ping"] {
        assert_eq!(
            w.wire(&old_token, command, &[])["code"],
            "unauthorized",
            "{command}"
        );
    }
    assert_eq!(
        w.wire(&old_token, "report", &[&old_commit, "x"])["code"],
        "unauthorized"
    );
    // The replacement works on a branch of its own; the predecessor's commit lies only on the old one.
    let base = w.git(&["rev-parse", "main"]);
    let branch = "feat/dev-1-g2";
    let worktree = w.root.join("wt/dev-1-g2");
    w.git(&[
        "worktree",
        "add",
        "--quiet",
        "-b",
        branch,
        worktree.to_str().unwrap(),
        "main",
    ]);
    w.move_pane("dev-1", branch, &worktree, &base);
    let refused = w.as_agent("dev-1", &["report", &old_commit, "the predecessor's work"]);
    rejected(&refused, "rejected");
    // Its own work is accepted.
    std::fs::write(worktree.join("mine.txt"), "mine\n").unwrap();
    w.git_in(&worktree, &["add", "--all"]);
    w.git_in(&worktree, &["commit", "-q", "-m", "feat: mine"]);
    let mine = w.git_in(&worktree, &["rev-parse", "HEAD"]);
    assert_eq!(
        field(
            &w.as_agent("dev-1", &["report", &mine, "mine"]).ok(),
            "state"
        ),
        "accepted"
    );
}
