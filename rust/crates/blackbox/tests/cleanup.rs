//! `cstan cleanup` through the executable: the dry run, the offer, apply and decline, and who may call it. The branch that
//! is done with is a developer's reported branch (and its worktree) after the integration is confirmed and the agent has
//! ended.

use capstan_blackbox::*;

fn world() -> World {
    World::builder("cleanup")
        .agents(&[
            agent("pm-1", "pm", "PM"),
            agent("dev-1", "developer", "Developer"),
            agent("dev-2", "developer", "Developer"),
            agent("rev-1", "reviewer", "Verifier"),
        ])
        .config(&format!("{TEAM_CONFIG}\n[cleanup]\nbranches = \"off\"\n"))
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

/// dev-1's branch is reported, reviewed, integrated into main and confirmed, and dev-1 has ended. Returns its commit and
/// worktree.
fn landed(w: &mut World) -> (String, std::path::PathBuf) {
    let (commit, worktree) = w.give_branch("dev-1", "one.txt", "one\n", "feat: one");
    let report = field(
        &w.as_agent("dev-1", &["report", &commit, "one"]).ok(),
        "reportId",
    );
    w.begin_review("pm-1", &report, "reviewer", "rev-1");
    w.as_agent("rev-1", &["review", "pass", "fine"]).ok();
    let merged = w.as_agent("pm-1", &["integrate", &report]).ok();
    let (integration, branch) = (field(&merged, "integrationId"), field(&merged, "branch"));
    w.begin_review("pm-1", &integration, "reviewer", "rev-1");
    w.as_agent("rev-1", &["review", "pass", "the merge is fine"])
        .ok();
    w.git(&["merge", "--ff-only", "--quiet", &branch]);
    w.as_agent("pm-1", &["integrate", "confirm", &integration])
        .ok();
    w.while_stopped(|core, ctx, owner| {
        core.end_agent(&ctx(owner), "dev-1", None).unwrap();
    });
    (commit, worktree)
}

#[test]
fn dry_run_prints_the_three_groups_and_changes_no_ref() {
    let mut w = world();
    let (commit, worktree) = landed(&mut w);
    let before = w.git(&["for-each-ref"]);
    let out = w.as_agent("pm-1", &["cleanup", "--dry-run"]).ok();
    assert_eq!(w.git(&["for-each-ref"]), before);
    for heading in ["Landed", "Unlanded", "Active or protected"] {
        assert!(out.stdout.contains(heading), "{heading}: {}", out.stdout);
    }
    let landed = out.stdout.split("Unlanded").next().unwrap();
    assert!(landed.contains(&format!("feat/dev-1 {commit}")), "{landed}");
    assert!(
        landed.contains(&format!("worktree: {}", worktree.display())),
        "{landed}"
    );
    assert!(landed.contains("reason: report"), "{landed}");
    assert!(
        out.stdout.contains("main"),
        "main is protected: {}",
        out.stdout
    );
    assert_eq!(
        w.op(&["cleanup", "--dry-run", "--json"]).ok().json()["dryRun"],
        true
    );
}

#[test]
fn apply_deletes_the_offered_branch_and_its_worktree_and_prints_the_restore_command() {
    let mut w = world();
    let (commit, worktree) = landed(&mut w);
    let offer = w.as_agent("pm-1", &["cleanup"]).ok();
    assert!(offer.stdout.contains("feat/dev-1"), "{}", offer.stdout);
    assert!(w
        .git(&["branch", "--list", "feat/dev-1"])
        .contains("feat/dev-1"));
    let id = offer
        .stdout
        .split("cleanup ")
        .nth(1)
        .and_then(|rest| rest.split(':').next())
        .unwrap()
        .to_string();
    let applied = w.as_agent("pm-1", &["cleanup", "apply", &id]).ok();
    assert!(
        applied
            .stdout
            .contains(&format!("deleted feat/dev-1 {commit}")),
        "{}",
        applied.stdout
    );
    assert!(
        applied
            .stdout
            .contains(&format!("git branch feat/dev-1 {commit}")),
        "{}",
        applied.stdout
    );
    assert!(w.git(&["branch", "--list", "feat/dev-1"]).is_empty());
    assert!(!worktree.exists(), "the worktree is removed");
    // An applied offer cannot be applied or declined again.
    w.as_agent("pm-1", &["cleanup", "apply", &id])
        .refused(4, "not_open");
    w.as_agent("pm-1", &["cleanup", "decline", &id])
        .refused(4, "not_open");
    // Nothing is left to offer.
    assert!(w
        .op(&["cleanup"])
        .ok()
        .stdout
        .contains("nothing to clean up"));
}

#[test]
fn decline_deletes_nothing() {
    let mut w = world();
    landed(&mut w);
    let offer = w.op(&["cleanup", "--json"]).ok().json();
    let id = offer["cleanupId"].as_str().unwrap().to_string();
    let declined = w.op(&["cleanup", "decline", &id]).ok();
    assert!(declined.stdout.contains("declined"), "{}", declined.stdout);
    assert!(w
        .git(&["branch", "--list", "feat/dev-1"])
        .contains("feat/dev-1"));
}

#[test]
fn a_worker_cannot_run_cleanup() {
    let mut w = world();
    landed(&mut w);
    for args in [
        vec!["cleanup"],
        vec!["cleanup", "--dry-run"],
        vec!["cleanup", "apply", "x"],
        vec!["cleanup", "decline", "x"],
    ] {
        let out = w.as_agent("dev-2", &args);
        assert_ne!(out.code, 0, "{}", out.text());
        assert!(out.has("only the PM or the operator"), "{}", out.text());
    }
    w.as_agent("rev-1", &["cleanup", "--dry-run"])
        .refused(4, "only the PM or the operator");
}
