//! The guarded branch cleanup on a scratch git repository and a real ledger: classification, the guards, offering,
//! applying (with the re-checks at apply time), the automatic run, the record, the restore commands and the budget.
//! Reports and integrations are laid down with SQL (what the pipeline would have recorded); everything else goes through
//! the kernel.

mod common;
#[path = "rust_only/fixture.rs"]
mod fixture;

use capstan_kernel::areas::cleanups;
use capstan_kernel::cleanup::{
    apply, apply_auto, auto_notice_text, classify, decline, offer, offer_notice_text, Class,
    CleanupBranches, CleanupDeps, CleanupError, CleanupSettings, CleanupWorktrees,
};
use capstan_kernel::integrate::{GitRepo, IntegrationState};
use fixture::{context, Fixture, OWNER};
use rusqlite::params;
use serde_json::{json, Value};
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const AUTO: CleanupSettings = CleanupSettings {
    branches: CleanupBranches::Auto,
    worktrees: CleanupWorktrees::Release,
    include_unlanded: false,
};
const AUTO_KEEP: CleanupSettings = CleanupSettings {
    worktrees: CleanupWorktrees::Keep,
    ..AUTO
};

struct World {
    fx: Fixture,
    repo: PathBuf,
    home: PathBuf,
    sequence: Cell<i64>,
    last_integration: std::cell::RefCell<String>,
}

impl World {
    fn new() -> World {
        let fx = Fixture::new();
        let repo = fx.dir.path().join("repo");
        let home = fx.dir.path().join("home");
        std::fs::create_dir(&repo).unwrap();
        std::fs::create_dir(&home).unwrap();
        let world = World {
            fx,
            repo,
            home,
            sequence: Cell::new(0),
            last_integration: Default::default(),
        };
        world.git(&["init", "-q", "-b", "main"]);
        world.write("base.txt", "base\n");
        world.commit("chore: base");
        world
    }

    fn git_in(&self, dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", &self.home)
            .env("LC_ALL", "C")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "scratch")
            .env("GIT_AUTHOR_EMAIL", "scratch@localhost")
            .env("GIT_COMMITTER_NAME", "scratch")
            .env("GIT_COMMITTER_EMAIL", "scratch@localhost")
            .stdin(Stdio::null())
            .output()
            .expect("git runs");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn git(&self, args: &[&str]) -> String {
        self.git_in(&self.repo, args)
    }

    fn write(&self, name: &str, content: &str) {
        std::fs::write(self.repo.join(name), content).unwrap();
    }

    fn commit(&self, message: &str) -> String {
        self.git(&["add", "-A"]);
        self.git(&[
            "commit",
            "-q",
            "--no-verify",
            "--no-gpg-sign",
            "-m",
            message,
        ]);
        self.git(&["rev-parse", "HEAD"]).trim().to_string()
    }

    /// A branch off main with one commit that adds `file`; main stays checked out.
    fn branch(&self, name: &str, file: &str, content: &str) -> String {
        self.git(&["checkout", "-q", "-b", name, "main"]);
        self.write(file, content);
        let sha = self.commit(&format!("feat: {name}"));
        self.git(&["checkout", "-q", "main"]);
        sha
    }

    fn tip(&self, branch: &str) -> Option<String> {
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.repo)
            .args([
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ])
            .output()
            .unwrap();
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    fn sql(&self, sql: &str, args: impl rusqlite::Params) {
        self.fx
            .core()
            .kernel()
            .database
            .connection()
            .execute(sql, args)
            .unwrap_or_else(|e| panic!("{sql}: {e}"));
    }

    fn next(&self) -> i64 {
        self.sequence.set(self.sequence.get() + 1);
        self.sequence.get()
    }

    /// An accepted report of dev-1 for `sha`.
    fn report(&self, sha: &str) -> String {
        let n = self.next();
        let id = format!("report-{n}");
        let actor: String = self
            .fx
            .core()
            .kernel()
            .database
            .connection()
            .query_row(
                "SELECT actor_id FROM agents WHERE agent_id = 'dev-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        self.sql(
            "INSERT INTO agent_reports(project_id, report_id, sequence, agent_id, generation, actor_id, commit_sha, branch, summary, state, evidence_json, created_at)
             VALUES ('prustonly', ?, ?, 'dev-1', 1, ?, ?, 'x', 'summary', 'accepted', '{}', '2026-01-01T00:00:00.000Z')",
            params![id, n, actor, sha],
        );
        id
    }

    /// An integration holding the reports, left in `state` (merged, confirmed or discarded).
    fn integration(&self, reports: &[&str], state: &str) -> String {
        let n = self.next();
        let id = format!("integration-{n}");
        self.sql(
            "INSERT INTO integrations(project_id, integration_id, sequence, base_sha, branch, requested_by, state, created_at)
             VALUES ('prustonly', ?, ?, ?, ?, 'operator', 'running', '2026-01-01T00:00:00.000Z')",
            params![id, n, "a".repeat(40), format!("capstan/integration/{id}")],
        );
        for (i, report) in reports.iter().enumerate() {
            self.sql(
                "INSERT INTO integration_reports(project_id, integration_id, position, report_id) VALUES ('prustonly', ?, ?, ?)",
                params![id, i as i64 + 1, report],
            );
        }
        self.sql(
            "UPDATE integrations SET state = 'merged', head_sha = ?, completed_at = '2026-01-01T00:00:01.000Z' WHERE integration_id = ?",
            params!["f".repeat(40), id],
        );
        if state != "merged" {
            self.sql(
                "UPDATE integrations SET state = ? WHERE integration_id = ?",
                params![state, id],
            );
        }
        *self.last_integration.borrow_mut() = id.clone();
        id
    }

    /// A branch whose tip is a report commit in a confirmed integration.
    fn landed_branch(&self, name: &str) -> String {
        let sha = self.branch(name, &format!("{name}.txt"), "x\n");
        let report = self.report(&sha);
        self.integration(&[&report], "confirmed");
        sha
    }

    fn last(&self) -> String {
        self.last_integration.borrow().clone()
    }

    fn pane(&self, branch: Option<&str>, worktree: Option<&Path>) {
        self.sql(
            "INSERT OR REPLACE INTO agent_panes(project_id, agent_id, branch, worktree_path, base_sha, generation, created_at, updated_at)
             VALUES ('prustonly', 'dev-1', ?, ?, ?, 1, '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z')",
            params![branch, worktree.map(|p| p.to_string_lossy().into_owned()), "a".repeat(40)],
        );
    }

    fn end_dev(&self) {
        let core = self.fx.core();
        core.end_agent(&context(core, OWNER), "dev-1", None)
            .unwrap();
    }

    fn run<R>(
        &self,
        settings: CleanupSettings,
        budget: Duration,
        now: &dyn Fn() -> Instant,
        f: impl FnOnce(&CleanupDeps) -> R,
    ) -> R {
        let git = GitRepo::new(&self.repo).with_env(vec![
            ("GIT_AUTHOR_NAME".into(), "scratch".into()),
            ("GIT_AUTHOR_EMAIL".into(), "scratch@localhost".into()),
            ("GIT_COMMITTER_NAME".into(), "scratch".into()),
            ("GIT_COMMITTER_EMAIL".into(), "scratch@localhost".into()),
        ]);
        let state = IntegrationState::new();
        let core = self.fx.core();
        let ctx = |credential: &str| context(core, credential);
        let log = |_: &str, _: Value| {};
        let deps = CleanupDeps {
            kernel: core.kernel(),
            git: &git,
            integrations: &state,
            credential: OWNER,
            context: &ctx,
            log: &log,
            settings,
            budget,
            now,
        };
        f(&deps)
    }

    fn with<R>(&self, settings: CleanupSettings, f: impl FnOnce(&CleanupDeps) -> R) -> R {
        self.run(settings, Duration::from_secs(3600), &Instant::now, f)
    }

    fn pause(&self) {
        let core = self.fx.core();
        core.transition_run(&context(core, OWNER), &json!("paused"), Some("test"))
            .unwrap();
    }
}

fn names(list: &[capstan_kernel::cleanup::Candidate]) -> Vec<&str> {
    list.iter().map(|c| c.branch.as_str()).collect()
}

fn protected<'a>(c: &'a capstan_kernel::cleanup::Classification, branch: &str) -> Option<&'a str> {
    c.protected
        .iter()
        .find(|p| p.branch == branch)
        .map(|p| p.reason)
}

fn offered_ids(w: &World) -> Vec<String> {
    let all = cleanups::cleanups(w.fx.core().kernel(), OWNER, Some(50)).unwrap();
    all.as_array()
        .unwrap()
        .iter()
        .map(|c| c["cleanupId"].as_str().unwrap().to_string())
        .collect()
}

// ---------------------------------------------------------------------------------------------- classification

#[test]
fn a_report_commit_in_a_confirmed_integration_is_landed() {
    let w = World::new();
    let sha = w.landed_branch("done");
    let c = w.with(AUTO, |d| classify(d).unwrap());
    assert_eq!(names(&c.landed), ["done"]);
    let entry = &c.landed[0];
    assert_eq!(
        (entry.class, entry.sha.as_str()),
        (Class::Landed, sha.as_str())
    );
    assert!(entry.report_id.is_some() && entry.integration_id.is_some());
}

#[test]
fn a_merged_or_discarded_integration_does_not_make_a_branch_landed() {
    let w = World::new();
    for (name, state) in [("merged-only", "merged"), ("thrown-away", "discarded")] {
        let sha = w.branch(name, &format!("{name}.txt"), "x\n");
        let report = w.report(&sha);
        w.integration(&[&report], state);
    }
    let c = w.with(AUTO, |d| classify(d).unwrap());
    assert!(c.landed.is_empty());
    assert_eq!(names(&c.unlanded), ["merged-only", "thrown-away"]);
}

#[test]
fn a_commit_after_the_report_commit_is_unlanded() {
    let w = World::new();
    w.landed_branch("grown");
    w.git(&["checkout", "-q", "grown"]);
    w.write("more.txt", "more\n");
    w.commit("feat: more");
    w.git(&["checkout", "-q", "main"]);
    let c = w.with(AUTO, |d| classify(d).unwrap());
    assert_eq!(names(&c.unlanded), ["grown"]);
}

#[test]
fn a_report_covered_by_a_confirmed_integration_is_landed() {
    let w = World::new();
    let sha = w.branch("covered", "c.txt", "c\n");
    let report = w.report(&sha);
    let integration = w.integration(&[], "confirmed");
    w.sql(
        "INSERT INTO integration_covered_reports(project_id, integration_id, report_id, how) VALUES ('prustonly', ?, ?, 'tree')",
        params![integration, report],
    );
    let c = w.with(AUTO, |d| classify(d).unwrap());
    assert_eq!(names(&c.landed), ["covered"]);
    assert_eq!(c.landed[0].class, Class::Landed);
}

#[test]
fn a_branch_whose_changes_are_all_in_head_is_landed_by_content() {
    let w = World::new();
    w.branch("pre-feature", "p.txt", "p\n");
    w.write("p.txt", "p\n");
    w.commit("feat: the same change by hand");
    let c = w.with(AUTO, |d| classify(d).unwrap());
    assert_eq!(names(&c.landed), ["pre-feature"]);
    assert_eq!(c.landed[0].class, Class::LandedByContent);
}

#[test]
fn a_cancelled_branch_that_was_never_integrated_is_unlanded() {
    let w = World::new();
    w.branch("cancelled", "z.txt", "z\n");
    let c = w.with(AUTO, |d| classify(d).unwrap());
    assert_eq!(names(&c.unlanded), ["cancelled"]);
    assert_eq!(c.unlanded[0].class, Class::Unlanded);
}

// ------------------------------------------------------------------------------------------------------ guards

#[test]
fn main_master_and_the_checked_out_branch_are_never_offered_or_deleted() {
    let w = World::new();
    w.git(&["branch", "master"]);
    w.landed_branch("done");
    w.git(&["checkout", "-q", "-b", "trunk"]);
    let c = w.with(AUTO, |d| classify(d).unwrap());
    assert_eq!(names(&c.landed), ["done"]);
    for branch in ["main", "master", "trunk"] {
        assert_eq!(protected(&c, branch), Some("main_or_head"), "{branch}");
    }
    let id = w.with(AUTO, |d| {
        offer(d, "manual", None, "operator", true)
            .unwrap()
            .cleanup_id
            .unwrap()
    });
    w.with(AUTO, |d| {
        apply(d, &id, Some(vec!["main".into(), "trunk".into()])).unwrap()
    });
    for branch in ["main", "master", "trunk"] {
        assert!(w.tip(branch).is_some(), "{branch} survives");
    }
}

#[test]
fn the_remote_default_branch_is_protected_and_remote_refs_are_untouched() {
    let w = World::new();
    w.git(&["branch", "develop"]);
    let main = w.tip("main").unwrap();
    w.git(&["update-ref", "refs/remotes/origin/develop", &main]);
    w.git(&["update-ref", "refs/remotes/origin/feature", &main]);
    w.git(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/develop",
    ]);
    w.landed_branch("done");
    let c = w.with(AUTO, |d| classify(d).unwrap());
    assert_eq!(protected(&c, "develop"), Some("main_or_head"));
    assert!(c
        .landed
        .iter()
        .chain(&c.unlanded)
        .all(|e| !e.branch.contains("origin")));
    w.with(AUTO, |d| apply_auto(d, &w.last()).unwrap());
    assert!(w.tip("done").is_none());
    let refs = w.git(&["for-each-ref", "--format=%(refname)", "refs/remotes"]);
    assert!(
        refs.contains("refs/remotes/origin/feature")
            && refs.contains("refs/remotes/origin/develop")
    );
}

#[test]
fn a_branch_in_a_dirty_worktree_is_protected_and_the_worktree_survives() {
    let w = World::new();
    let sha = w.landed_branch("busy");
    let tree = w.fx.dir.path().join("wt-busy");
    w.git(&["worktree", "add", "-q", tree.to_str().unwrap(), "busy"]);
    std::fs::write(tree.join("scratch.txt"), "unsaved\n").unwrap();
    w.pane(Some("busy"), Some(&tree));
    w.end_dev();
    let c = w.with(AUTO, |d| classify(d).unwrap());
    assert_eq!(protected(&c, "busy"), Some("dirty_worktree"));
    assert!(w
        .with(AUTO, |d| apply_auto(d, &w.last()).unwrap())
        .deleted
        .is_empty());
    assert_eq!(w.tip("busy").as_deref(), Some(sha.as_str()));
    assert!(tree.join("scratch.txt").exists());
}

#[test]
fn an_active_agents_branch_is_protected() {
    let w = World::new();
    w.landed_branch("mine");
    w.pane(Some("mine"), None);
    let c = w.with(AUTO, |d| classify(d).unwrap());
    assert_eq!(protected(&c, "mine"), Some("active_agent"));
    assert!(w
        .with(AUTO, |d| apply_auto(d, &w.last()).unwrap())
        .deleted
        .is_empty());
    assert!(w.tip("mine").is_some());
}

#[test]
fn a_branch_checked_out_in_a_worktree_nobody_recorded_is_protected() {
    let w = World::new();
    w.landed_branch("elsewhere");
    let tree = w.fx.dir.path().join("wt-else");
    w.git(&["worktree", "add", "-q", tree.to_str().unwrap(), "elsewhere"]);
    let c = w.with(AUTO, |d| classify(d).unwrap());
    assert_eq!(protected(&c, "elsewhere"), Some("checked_out"));
}

#[test]
fn an_unsettled_integration_branch_is_protected() {
    let w = World::new();
    let id = w.integration(&[], "merged");
    let main = w.tip("main").unwrap();
    w.git(&[
        "update-ref",
        &format!("refs/heads/capstan/integration/{id}"),
        &main,
    ]);
    let c = w.with(AUTO, |d| classify(d).unwrap());
    assert_eq!(
        protected(&c, &format!("capstan/integration/{id}")),
        Some("integration_branch_unsettled")
    );
}

#[test]
fn offer_and_apply_are_refused_while_paused_and_while_an_integration_runs() {
    let w = World::new();
    w.landed_branch("done");
    let id = w.with(AUTO, |d| {
        offer(d, "manual", None, "operator", false)
            .unwrap()
            .cleanup_id
            .unwrap()
    });
    // An integration is running.
    let n = w.next();
    w.sql(
        "INSERT INTO integrations(project_id, integration_id, sequence, base_sha, branch, requested_by, state, created_at)
         VALUES ('prustonly', 'running-one', ?, ?, 'capstan/integration/running-one', 'operator', 'running', '2026-01-01T00:00:00.000Z')",
        params![n, "a".repeat(40)],
    );
    let refused = |e: CleanupError, want: &str| match e {
        CleanupError::Refused { code, message } => {
            assert_eq!(code, want);
            assert!(message.starts_with("cleanup_refused:"), "{message}");
        }
        other => panic!("{other:?}"),
    };
    refused(
        w.with(AUTO, |d| offer(d, "manual", None, "operator", false))
            .unwrap_err(),
        "integration_running",
    );
    refused(
        w.with(AUTO, |d| apply(d, &id, None)).unwrap_err(),
        "integration_running",
    );
    refused(
        w.with(AUTO, |d| apply_auto(d, &w.last())).unwrap_err(),
        "integration_running",
    );
    w.sql(
        "UPDATE integrations SET state = 'failed', failure_reason = 'gone', completed_at = '2026-01-01T00:00:02.000Z' WHERE integration_id = 'running-one'",
        [],
    );
    w.pause();
    refused(
        w.with(AUTO, |d| offer(d, "manual", None, "operator", false))
            .unwrap_err(),
        "paused",
    );
    refused(w.with(AUTO, |d| apply(d, &id, None)).unwrap_err(), "paused");
    assert!(w.tip("done").is_some());
}

// -------------------------------------------------------------------------------------- re-checks at apply time

#[test]
fn a_branch_whose_tip_moved_after_the_offer_is_skipped() {
    let w = World::new();
    w.landed_branch("moves");
    let id = w.with(AUTO, |d| {
        offer(d, "manual", None, "operator", false)
            .unwrap()
            .cleanup_id
            .unwrap()
    });
    w.git(&["checkout", "-q", "moves"]);
    w.write("later.txt", "later\n");
    let moved = w.commit("feat: later");
    w.git(&["checkout", "-q", "main"]);
    let applied = w.with(AUTO, |d| apply(d, &id, None).unwrap());
    assert!(applied.deleted.is_empty());
    assert_eq!(applied.skipped[0].reason, "tip_moved");
    assert_eq!(w.tip("moves").as_deref(), Some(moved.as_str()));
}

#[test]
fn an_agent_that_took_the_branch_after_the_offer_is_not_overridden() {
    let w = World::new();
    w.landed_branch("taken");
    let id = w.with(AUTO, |d| {
        offer(d, "manual", None, "operator", false)
            .unwrap()
            .cleanup_id
            .unwrap()
    });
    w.pane(Some("taken"), None);
    let applied = w.with(AUTO, |d| apply(d, &id, None).unwrap());
    assert_eq!(applied.skipped[0].reason, "now_active");
    assert!(w.tip("taken").is_some());
}

#[test]
fn a_lost_report_link_is_a_changed_class() {
    let w = World::new();
    let sha = w.branch("linked", "l.txt", "l\n");
    let report = w.report(&sha);
    let integration = w.integration(&[], "confirmed");
    w.sql(
        "INSERT INTO integration_covered_reports(project_id, integration_id, report_id, how) VALUES ('prustonly', ?, ?, 'tree')",
        params![integration, report],
    );
    let id = w.with(AUTO, |d| {
        offer(d, "manual", None, "operator", false)
            .unwrap()
            .cleanup_id
            .unwrap()
    });
    w.sql(
        "DELETE FROM integration_covered_reports WHERE report_id = ?",
        params![report],
    );
    let applied = w.with(AUTO, |d| apply(d, &id, None).unwrap());
    assert_eq!(applied.skipped[0].reason, "class_changed");
    assert!(w.tip("linked").is_some());
}

#[test]
fn a_landed_by_content_branch_whose_content_left_head_is_a_changed_class() {
    let w = World::new();
    w.branch("by-content", "p.txt", "p\n");
    w.write("p.txt", "p\n");
    w.commit("feat: same by hand");
    let id = w.with(AUTO, |d| {
        offer(d, "manual", None, "operator", false)
            .unwrap()
            .cleanup_id
            .unwrap()
    });
    w.write("p.txt", "changed since\n");
    w.commit("feat: head moves on");
    let applied = w.with(AUTO, |d| {
        apply(d, &id, Some(vec!["by-content".into()])).unwrap()
    });
    assert_eq!(applied.skipped[0].reason, "class_changed");
    assert!(w.tip("by-content").is_some());
}

// ------------------------------------------------------------------------------------------------- selection

#[test]
fn apply_deletes_landed_entries_by_default_and_the_others_only_when_named() {
    let w = World::new();
    w.landed_branch("landed-one");
    w.branch("by-content", "p.txt", "p\n");
    w.write("p.txt", "p\n");
    w.commit("feat: same by hand");
    w.branch("lost", "z.txt", "z\n");
    let offered = w.with(AUTO, |d| {
        offer(d, "manual", None, "operator", true).unwrap()
    });
    assert_eq!(
        names(&offered.entries),
        ["by-content", "landed-one", "lost"]
    );
    let id = offered.cleanup_id.unwrap();
    let applied = w.with(AUTO, |d| apply(d, &id, None).unwrap());
    assert_eq!(applied.deleted.len(), 1);
    assert!(
        w.tip("landed-one").is_none() && w.tip("by-content").is_some() && w.tip("lost").is_some()
    );

    let again = w.with(AUTO, |d| {
        offer(d, "manual", None, "operator", true).unwrap()
    });
    let id = again.cleanup_id.unwrap();
    let applied = w.with(AUTO, |d| {
        apply(
            d,
            &id,
            Some(vec!["by-content".into(), "lost".into(), "ghost".into()]),
        )
        .unwrap()
    });
    assert_eq!(applied.deleted.len(), 2);
    assert_eq!(applied.skipped[0].reason, "not_offered");
    assert!(w.tip("by-content").is_none() && w.tip("lost").is_none());
}

#[test]
fn unlanded_branches_are_offered_only_when_asked_for() {
    let w = World::new();
    w.branch("lost", "z.txt", "z\n");
    assert!(w
        .with(AUTO, |d| offer(d, "manual", None, "operator", false)
            .unwrap())
        .cleanup_id
        .is_none());
    assert!(w
        .with(AUTO, |d| offer(d, "manual", None, "operator", true)
            .unwrap())
        .cleanup_id
        .is_some());
}

#[test]
fn the_automatic_run_deletes_landed_branches_only() {
    let w = World::new();
    w.landed_branch("landed-one");
    w.branch("by-content", "p.txt", "p\n");
    w.write("p.txt", "p\n");
    w.commit("feat: same by hand");
    w.branch("lost", "z.txt", "z\n");
    let applied = w.with(AUTO, |d| apply_auto(d, &w.last()).unwrap());
    assert_eq!(applied.deleted.len(), 1);
    assert!(
        w.tip("landed-one").is_none() && w.tip("by-content").is_some() && w.tip("lost").is_some()
    );
    let record =
        cleanups::cleanup(w.fx.core().kernel(), applied.cleanup_id.as_deref().unwrap()).unwrap();
    assert_eq!(
        (record["mode"].as_str(), record["state"].as_str()),
        (Some("auto"), Some("applied"))
    );
    assert!(auto_notice_text(&applied).contains("landed-one"));
    // Not in automatic mode: refused.
    let ask = CleanupSettings {
        branches: CleanupBranches::Ask,
        ..AUTO
    };
    assert!(matches!(
        w.with(ask, |d| apply_auto(d, &w.last())),
        Err(CleanupError::Refused {
            code: "not_auto",
            ..
        })
    ));
}

#[test]
fn an_ended_agents_clean_worktree_is_removed_by_release_and_kept_by_keep() {
    for (settings, removed) in [(AUTO, true), (AUTO_KEEP, false)] {
        let w = World::new();
        let sha = w.landed_branch("worked");
        let tree = w.fx.dir.path().join("wt-worked");
        w.git(&["worktree", "add", "-q", tree.to_str().unwrap(), "worked"]);
        w.pane(Some("worked"), Some(&tree));
        w.end_dev();
        let c = w.with(settings, |d| classify(d).unwrap());
        assert_eq!(
            c.landed[0]
                .worktree_path
                .as_deref()
                .map(Path::new)
                .map(|p| p.canonicalize().unwrap()),
            Some(tree.canonicalize().unwrap())
        );
        let applied = w.with(settings, |d| apply_auto(d, &w.last()).unwrap());
        assert_eq!(applied.deleted.len(), usize::from(removed));
        assert_eq!(tree.exists(), !removed);
        assert_eq!(w.tip("worked").is_none(), removed);
        if !removed {
            assert_eq!(w.tip("worked").as_deref(), Some(sha.as_str()));
            // An offer still lists it, and applying that offer by hand removes the worktree.
            let offered = w.with(settings, |d| {
                offer(d, "manual", None, "operator", false).unwrap()
            });
            assert_eq!(names(&offered.entries), ["worked"]);
            w.with(settings, |d| {
                apply(d, offered.cleanup_id.as_deref().unwrap(), None).unwrap()
            });
            assert!(!tree.exists() && w.tip("worked").is_none());
        }
    }
}

// ----------------------------------------------------------------------------------- record, restore, offers

#[test]
fn every_deletion_is_recorded_and_the_restore_command_brings_the_branch_back() {
    let w = World::new();
    let sha = w.landed_branch("gone");
    let id = w.with(AUTO, |d| {
        offer(d, "manual", None, "operator", false)
            .unwrap()
            .cleanup_id
            .unwrap()
    });
    let applied = w.with(AUTO, |d| apply(d, &id, None).unwrap());
    assert_eq!(applied.restore, [format!("git branch gone {sha}")]);
    assert!(w.tip("gone").is_none());
    let record = cleanups::cleanup(w.fx.core().kernel(), &id).unwrap();
    assert_eq!(record["state"], "applied");
    let entry = &record["entries"][0];
    assert_eq!(
        (
            entry["branch"].as_str(),
            entry["sha"].as_str(),
            entry["outcome"].as_str()
        ),
        (Some("gone"), Some(sha.as_str()), Some("deleted"))
    );
    let args: Vec<&str> = applied.restore[0].split(' ').skip(1).collect();
    w.git(&args);
    assert_eq!(w.tip("gone").as_deref(), Some(sha.as_str()));
    // The record cannot be taken back.
    let database = w.fx.core().kernel().database.connection();
    assert!(database
        .execute("DELETE FROM cleanup_branches", [])
        .is_err());
    assert!(database
        .execute("UPDATE cleanup_branches SET outcome = 'offered'", [])
        .is_err());
    assert!(database
        .execute("UPDATE cleanups SET state = 'offered'", [])
        .is_err());
}

#[test]
fn a_superseded_offer_cannot_be_applied_and_decline_deletes_nothing() {
    let w = World::new();
    w.landed_branch("done");
    let first = w.with(AUTO, |d| {
        offer(d, "manual", None, "operator", false)
            .unwrap()
            .cleanup_id
            .unwrap()
    });
    let second = w.with(AUTO, |d| {
        offer(d, "manual", None, "operator", false)
            .unwrap()
            .cleanup_id
            .unwrap()
    });
    let refused = w.with(AUTO, |d| apply(d, &first, None)).unwrap_err();
    assert!(
        matches!(
            refused,
            CleanupError::Refused {
                code: "not_open",
                ..
            }
        ),
        "{refused:?}"
    );
    assert!(w.tip("done").is_some());
    w.with(AUTO, |d| decline(d, &second).unwrap());
    assert!(w.tip("done").is_some());
    assert!(matches!(
        w.with(AUTO, |d| apply(d, &second, None)),
        Err(CleanupError::Refused {
            code: "not_open",
            ..
        })
    ));
    assert!(matches!(
        w.with(AUTO, |d| apply(d, "nope", None)),
        Err(CleanupError::Refused {
            code: "unknown_cleanup",
            ..
        })
    ));
    let states: Vec<String> = offered_ids(&w)
        .iter()
        .map(|id| {
            cleanups::cleanup(w.fx.core().kernel(), id).unwrap()["state"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(states, ["declined", "superseded"]);
}

#[test]
fn the_notice_names_the_branch_the_commit_the_id_and_the_commands() {
    let w = World::new();
    let sha = w.landed_branch("done");
    let offered = w.with(AUTO, |d| {
        offer(d, "confirm", Some(w.last().as_str()), "operator", false).unwrap()
    });
    let id = offered.cleanup_id.clone().unwrap();
    let text = offer_notice_text(&offered);
    for part in [
        "done",
        sha.as_str(),
        id.as_str(),
        "cstan cleanup apply",
        "cstan cleanup decline",
    ] {
        assert!(text.contains(part), "{part} in {text}");
    }
    assert_eq!(offered.notice.as_deref(), Some(text.as_str()));
    // A confirm offer is announced to the PM as an action-needed message.
    let record = cleanups::cleanup(w.fx.core().kernel(), &id).unwrap();
    let message = record["messageId"]
        .as_str()
        .expect("the notice was queued")
        .to_string();
    let needed: i64 =
        w.fx.core()
            .kernel()
            .database
            .connection()
            .query_row(
                "SELECT action_needed FROM messages WHERE message_id = ?",
                [&message],
                |r| r.get(0),
            )
            .unwrap();
    assert_eq!(needed, 1);
}

// ------------------------------------------------------------------------------------------------------ budget

#[test]
fn a_budget_that_expires_mid_apply_skips_the_rest_and_keeps_what_was_done() {
    let w = World::new();
    w.landed_branch("a-first");
    w.landed_branch("b-second");
    w.landed_branch("c-third");
    let id = w.with(AUTO, |d| {
        offer(d, "manual", None, "operator", false)
            .unwrap()
            .cleanup_id
            .unwrap()
    });
    let start = Instant::now();
    // The clock jumps past the budget as soon as the first branch is gone.
    let clock = || {
        if w.tip("a-first").is_none() {
            start + Duration::from_secs(100)
        } else {
            start
        }
    };
    let applied = w.run(AUTO, Duration::from_secs(10), &clock, |d| {
        apply(d, &id, None).unwrap()
    });
    assert_eq!(applied.deleted.len(), 1);
    assert_eq!(applied.deleted[0].branch, "a-first");
    assert_eq!(
        applied
            .skipped
            .iter()
            .map(|s| (s.branch.as_str(), s.reason.as_str()))
            .collect::<Vec<_>>(),
        [("b-second", "budget"), ("c-third", "budget")]
    );
    assert!(w.tip("b-second").is_some() && w.tip("c-third").is_some());
    let record = cleanups::cleanup(w.fx.core().kernel(), &id).unwrap();
    let outcomes: Vec<(&str, &str)> = record["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["branch"].as_str().unwrap(),
                e["outcome"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        outcomes,
        [
            ("a-first", "deleted"),
            ("b-second", "skipped"),
            ("c-third", "skipped")
        ]
    );
}

// ------------------------------------------------------------------------------------------------------ harness

#[test]
fn the_table_dump_ignores_migration_38_and_empty_cleanup_tables_but_not_cleanup_rows() {
    let mut w = World::new();
    w.fx.close();
    let file = w.fx.database();
    let dump = common::replay::dump_tables(&file);
    let versions = dump["schema_migrations"]["rows"].as_array().unwrap();
    assert_eq!(versions.last().unwrap()[0], 37, "the 38 row is left out");
    assert!(dump.get("cleanups").is_none() && dump.get("cleanup_branches").is_none());
    w.fx.reopen();
    w.landed_branch("done");
    w.with(AUTO, |d| {
        offer(d, "manual", None, "operator", false).unwrap()
    });
    w.fx.close();
    let dump = common::replay::dump_tables(&file);
    assert_eq!(
        dump["cleanups"]["rows"].as_array().unwrap().len(),
        1,
        "a cleanup row stays in the dump"
    );
    assert!(dump.get("cleanup_branches").is_some());
}
