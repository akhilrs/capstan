//! The squash subject of a report is its first commit's, not its tip's.

use capstan_kernel::integrate::GitRepo;
use std::process::Command;

fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "{args:?}");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

#[test]
fn a_report_of_two_commits_names_the_first_and_a_single_commit_its_own() {
    let dir = std::env::temp_dir().join(format!("first-subject-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q", "-b", "main"]);
    git(
        &dir,
        &["commit", "-q", "--allow-empty", "-m", "chore: base"],
    );
    let base = git(&dir, &["rev-parse", "HEAD"]);
    git(
        &dir,
        &["commit", "-q", "--allow-empty", "-m", "docs(readme): first"],
    );
    let first = git(&dir, &["rev-parse", "HEAD"]);
    git(
        &dir,
        &["commit", "-q", "--allow-empty", "-m", "fix(readme): second"],
    );
    let tip = git(&dir, &["rev-parse", "HEAD"]);
    let repo = GitRepo::new(&dir);
    let subject = |sha: &str| repo.first_commit_subject(&base, sha).unwrap();
    assert_eq!(subject(&tip).as_deref(), Some("docs(readme): first"));
    assert_eq!(subject(&first).as_deref(), Some("docs(readme): first"));
    // A rework stacked on an earlier unmerged report keeps the original first commit's subject, and a later stacked
    // report does not change the earlier one's.
    git(
        &dir,
        &["commit", "-q", "--allow-empty", "-m", "fix(readme): rework"],
    );
    let rework = git(&dir, &["rev-parse", "HEAD"]);
    assert_eq!(subject(&rework).as_deref(), Some("docs(readme): first"));
    assert_eq!(subject(&tip).as_deref(), Some("docs(readme): first"));
    // Work the base already holds is not counted.
    let advanced = git(&dir, &["rev-parse", "HEAD~1"]);
    let repo_advanced = repo.first_commit_subject(&advanced, &rework).unwrap();
    assert_eq!(repo_advanced.as_deref(), Some("fix(readme): rework"));
    assert_eq!(subject(&base).as_deref(), Some("chore: base"));
    let _ = std::fs::remove_dir_all(&dir);
}
