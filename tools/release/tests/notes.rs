//! The cases of the former test/release-notes.test.ts.
use std::path::Path;
use std::process::Command;

use capstan_release::*;

fn c(message: &str) -> Commit {
    Commit::new("", message)
}

fn msgs(list: &[&str]) -> Vec<Commit> {
    list.iter().map(|m| c(m)).collect()
}

fn next(current: &str, list: &[&str]) -> Option<(String, &'static str)> {
    next_version(current, &msgs(list)).unwrap().map(|n| (n.version, n.level))
}

#[test]
fn level_fix_and_perf_are_patch_feat_is_minor_breaking_is_major() {
    assert_eq!(next("1.2.3", &["fix: a"]), Some(("1.2.4".into(), "patch")));
    assert_eq!(next("1.2.3", &["perf(x): a"]).unwrap().1, "patch");
    assert_eq!(next("1.2.3", &["fix: a", "feat: b"]), Some(("1.3.0".into(), "minor")));
    assert_eq!(next("1.2.3", &["feat!: b", "feat: c"]), Some(("2.0.0".into(), "major")));
    assert_eq!(next("1.2.3", &["fix: a\n\nBREAKING CHANGE: gone"]).unwrap().1, "major");
}

#[test]
fn level_breaking_on_0_y_z_is_minor() {
    assert_eq!(next("0.1.1", &["feat!: a"]), Some(("0.2.0".into(), "minor")));
    assert_eq!(next("0.1.1", &["feat: a"]), Some(("0.2.0".into(), "minor")));
}

#[test]
fn level_only_docs_chore_commits_merges_and_junk_give_none() {
    assert_eq!(next("1.0.0", &["docs: a", "chore: b", "test: c", "ci: d"]), None);
    assert_eq!(next("1.0.0", &["Merge branch 'x'", "fixed stuff"]), None);
    assert_eq!(next("1.0.0", &["Merge branch 'x'", "fix: real"]).unwrap().1, "patch");
}

#[test]
fn parse_commit_reads_scope_breaking_footer_and_rejects_junk() {
    assert_eq!(
        parse_commit("feat(ui)!: big"),
        Some(Parsed {
            kind: "feat".into(),
            scope: Some("ui".into()),
            breaking: true,
            description: "big".into(),
            note: None
        })
    );
    assert!(parse_commit("fix: a\n\nBREAKING-CHANGE: x").unwrap().breaking);
    assert_eq!(parse_commit("Merge pull request #1"), None);
}

#[test]
fn accepts_and_rejects_the_subjects_src_conventions_does() {
    // The table src/conventions.ts parseCommitSubject gives for these (the Node test compared the two).
    let cases = [
        ("feat: add", true),
        ("fix(core): bug", true),
        ("feat(ui)!: big", true),
        ("revert: undo", true),
        ("style: fmt", true),
        ("Feat: caps", false),
        ("feat:nospace", false),
        ("feat:  two spaces", false),
        ("feat(): empty", false),
        ("feat(a b): space", false),
        ("wip: nope", false),
        ("feature: nope", false),
        ("feat: ", false),
        ("Merge branch 'main'", false),
        ("", false),
    ];
    for (subject, ok) in cases {
        assert_eq!(parse_commit(subject).is_some(), ok, "{subject:?}");
    }
}

#[test]
fn changelog_section_groups_by_type_with_scope_and_short_sha() {
    let sha = "abcdef0123456789";
    let list = [
        "chore: tidy",
        "feat(ui): shiny",
        "revert: undo x",
        "perf: quick",
        "fix: bug",
        "feat!: drop\n\nBREAKING CHANGE: old api removed",
        "docs: words",
    ];
    let commits: Vec<Commit> = list.iter().map(|m| Commit::new(sha, m)).collect();
    let section = render_changelog_section("2.0.0", "2026-10-04", &commits);
    let want = [
        "## 2.0.0 (2026-10-04)",
        "",
        "### Breaking changes",
        "",
        "- old api removed (abcdef0)",
        "",
        "### Features",
        "",
        "- **ui:** shiny (abcdef0)",
        "",
        "### Bug fixes",
        "",
        "- bug (abcdef0)",
        "",
        "### Performance",
        "",
        "- quick (abcdef0)",
        "",
        "### Reverts",
        "",
        "- undo x (abcdef0)",
        "",
        "### Documentation",
        "",
        "- words (abcdef0)",
        "",
        "### Chores",
        "",
        "- tidy (abcdef0)",
        "",
    ]
    .join("\n");
    assert_eq!(section, want);
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(["-c", "user.email=t@t", "-c", "user.name=t", "-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git runs");
    assert!(status.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&status.stderr));
}

#[test]
fn last_release_tag_picks_the_highest_vxyz_tag_reachable_from_head() {
    let dir = std::env::temp_dir().join(format!("capstan-release-tags-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q"]);
    assert_eq!(last_release_tag(&dir), None);
    git(&dir, &["commit", "-q", "--allow-empty", "-m", "chore: a"]);
    for tag in ["v0.9.0", "v0.10.0", "vnext", "v0.10.0-rc1"] {
        git(&dir, &["tag", tag]);
    }
    git(&dir, &["commit", "-q", "--allow-empty", "-m", "chore: b"]);
    git(&dir, &["tag", "v0.2.0"]);
    assert_eq!(last_release_tag(&dir).as_deref(), Some("v0.10.0"));
    git(&dir, &["checkout", "-q", "-b", "side", "HEAD~1"]);
    git(&dir, &["commit", "-q", "--allow-empty", "-m", "chore: c"]);
    git(&dir, &["tag", "v5.0.0"]);
    git(&dir, &["checkout", "-q", "-"]);
    assert_eq!(last_release_tag(&dir).as_deref(), Some("v0.10.0"));
    std::fs::remove_dir_all(&dir).unwrap();
}

fn count(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

#[test]
fn changelog_drops_an_exact_duplicate_and_keeps_the_first() {
    let section = render_changelog_section(
        "1.0.0",
        "2026-10-04",
        &[Commit::new("aaaaaaa1", "fix(ui): same thing"), Commit::new("bbbbbbb2", "fix(ui): same thing")],
    );
    assert_eq!(count(&section, "same thing"), 1);
    assert!(section.contains("aaaaaaa"));
}

#[test]
fn changelog_treats_case_spacing_and_trailing_punctuation_as_the_same_subject() {
    let section = render_changelog_section(
        "1.0.0",
        "2026-10-04",
        &[Commit::new("aaaaaaa1", "feat: Add The Thing."), Commit::new("bbbbbbb2", "feat: add the  thing ")],
    );
    assert_eq!(count(&section, "- "), 1);
}

#[test]
fn changelog_keeps_different_subjects_that_share_a_short_prefix() {
    let commits = [
        Commit::new("aaaaaaa1", "fix: handle null"),
        Commit::new("bbbbbbb2", "fix: handle null in parser"),
        Commit::new("ccccccc3", "fix: handle timeouts"),
    ];
    assert_eq!(dedupe_commits(&commits).len(), 3);
    assert_eq!(count(&render_changelog_section("1.0.0", "2026-10-04", &commits), "- "), 3);
}

#[test]
fn changelog_merges_a_scoped_cut_off_and_retyped_repeat_into_one_entry() {
    let title = "Split oversized modules and remove the dormant legacy engine";
    let commits = [
        Commit::new("aaaaaaa1", "refactor(controller): Split oversized modules and remove the dormant"),
        Commit::new("bbbbbbb2", &format!("refactor: {title}")),
        Commit::new("ccccccc3", &format!("chore: {title}")),
        Commit::new("ddddddd4", &format!("feat(ui): {title}.")),
    ];
    let merged = dedupe_commits(&commits);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].parsed.kind, "feat");
    assert_eq!(merged[0].parsed.scope.as_deref(), Some("controller"));
    assert_eq!(merged[0].parsed.description, format!("{title}."));
    let section = render_changelog_section("1.0.0", "2026-10-04", &commits);
    assert_eq!(count(&section, "- "), 1);
    assert!(section.contains("### Features"));
}

#[test]
fn changelog_keeps_a_scope_and_breaking_flag_from_either_repeat() {
    let merged = dedupe_commits(&[
        Commit::new("aaaaaaa1", "fix(cli): a long enough shared subject line here"),
        Commit::new("bbbbbbb2", "fix!: a long enough shared subject line here"),
    ]);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].parsed.scope.as_deref(), Some("cli"));
    assert!(merged[0].parsed.breaking);
}
