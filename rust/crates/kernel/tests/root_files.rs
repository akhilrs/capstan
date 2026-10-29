//! The repository file checks of a merge (alternates, grafts, shallow) and the tie of the config docs to the code lists.

use capstan_config::remote::DOC_UNSAFE_FAMILIES;
use capstan_kernel::integrate::{
    GitRepo, GitSeams, UnsafeConfig, UnsafeScope, MERGE_AUDITED_COMMANDS, MERGE_UNSAFE_FAMILIES,
};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Fx {
    tmp: tempfile::TempDir,
    root: PathBuf,
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn fixture() -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("root");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["-c", "init.defaultBranch=main", "init", "-q"]);
    Fx { tmp, root }
}

impl Fx {
    fn repo_at(&self, root: &Path) -> GitRepo {
        let home = self.tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let home = home.to_string_lossy().into_owned();
        GitRepo::new(root).with_seams(GitSeams {
            forward_env: Some(vec![
                ("HOME".into(), home.clone()),
                ("XDG_CONFIG_HOME".into(), home.clone()),
                ("GIT_CONFIG_GLOBAL".into(), format!("{home}/gitconfig")),
                ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
            ]),
            ..GitSeams::default()
        })
    }
    fn check_at(&self, root: &Path, scope: UnsafeScope) -> UnsafeConfig {
        self.repo_at(root).unsafe_repo_config(scope).unwrap()
    }
    fn check(&self, scope: UnsafeScope) -> UnsafeConfig {
        self.check_at(&self.root, scope)
    }
    fn write(&self, rel: &str, content: &str) {
        let path = self.root.join(".git").join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
}

fn refused(config: UnsafeConfig) -> Vec<String> {
    match config {
        UnsafeConfig::Keys(keys) => keys,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

const ALTERNATES: &str = ".git/objects/info/alternates";

#[test]
fn a_file_with_a_line_refuses_merge_and_push_ignores_it() {
    for (rel, shown) in [
        ("objects/info/alternates", ALTERNATES),
        ("info/grafts", ".git/info/grafts"),
        ("shallow", ".git/shallow"),
    ] {
        let fx = fixture();
        fx.write(rel, "/some/path\n");
        assert_eq!(refused(fx.check(UnsafeScope::Merge)), [shown]);
        assert_eq!(fx.check(UnsafeScope::Push), UnsafeConfig::Clean, "{rel}");
    }
}

#[test]
fn an_alternates_directory_refuses() {
    let fx = fixture();
    std::fs::create_dir_all(fx.root.join(".git").join("objects/info/alternates")).unwrap();
    assert_eq!(refused(fx.check(UnsafeScope::Merge)), [ALTERNATES]);
}

#[test]
fn empty_and_comment_only_files_are_clean() {
    let fx = fixture();
    for content in [
        "",
        "\n\n",
        "  \t\n \r\n",
        "# nothing\n",
        "\r\n\r\n",
        "# a\n\n# b\n",
    ] {
        fx.write("objects/info/alternates", content);
        fx.write("info/grafts", content);
        assert_eq!(
            fx.check(UnsafeScope::Merge),
            UnsafeConfig::Clean,
            "{content:?}"
        );
    }
}

#[test]
fn a_crlf_path_line_refuses() {
    let fx = fixture();
    fx.write("objects/info/alternates", "/path\r\n");
    assert_eq!(refused(fx.check(UnsafeScope::Merge)), [ALTERNATES]);
}

#[test]
fn symlinks_are_followed_and_a_dangling_one_refuses() {
    let fx = fixture();
    let target = fx.tmp.path().join("target");
    let link = fx.root.join(".git/objects/info/alternates");
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();
    assert_eq!(refused(fx.check(UnsafeScope::Merge)), [ALTERNATES]);
    std::fs::write(&target, "# comment\n").unwrap();
    assert_eq!(fx.check(UnsafeScope::Merge), UnsafeConfig::Clean);
    std::fs::write(&target, "/path\n").unwrap();
    assert_eq!(refused(fx.check(UnsafeScope::Merge)), [ALTERNATES]);
}

fn linked_worktree(fx: &Fx) -> PathBuf {
    git(&fx.root, &["config", "user.name", "a"]);
    git(&fx.root, &["config", "user.email", "a@b.c"]);
    git(&fx.root, &["commit", "-q", "--allow-empty", "-m", "init"]);
    let linked = fx.tmp.path().join("linked");
    git(
        &fx.root,
        &[
            "worktree",
            "add",
            "-q",
            linked.to_str().unwrap(),
            "-b",
            "other",
        ],
    );
    linked
}

#[test]
fn a_linked_worktree_checks_the_common_dir_only() {
    let fx = fixture();
    let linked = linked_worktree(&fx);
    assert_eq!(
        fx.check_at(&linked, UnsafeScope::Merge),
        UnsafeConfig::Clean
    );
    let private = fx.root.join(".git/worktrees/linked");
    std::fs::create_dir_all(private.join("objects/info")).unwrap();
    std::fs::write(private.join("objects/info/alternates"), "/p\n").unwrap();
    std::fs::write(private.join("shallow"), "abc\n").unwrap();
    assert_eq!(
        fx.check_at(&linked, UnsafeScope::Merge),
        UnsafeConfig::Clean
    );
    for (rel, name) in [
        ("objects/info/alternates", "objects/info/alternates"),
        ("info/grafts", "info/grafts"),
        ("shallow", "shallow"),
    ] {
        let fx = fixture();
        let linked = linked_worktree(&fx);
        fx.write(rel, "/p\n");
        let common = fx.root.join(".git").join(name);
        let found = refused(fx.check_at(&linked, UnsafeScope::Merge));
        let shown = common.canonicalize().unwrap_or(common);
        assert_eq!(found.len(), 1, "{rel}: {found:?}");
        assert!(
            Path::new(&found[0]).canonicalize().unwrap() == shown,
            "{rel}: {found:?}"
        );
        assert_eq!(fx.check_at(&linked, UnsafeScope::Push), UnsafeConfig::Clean);
    }
}

#[test]
fn the_audited_commands_are_the_fifteen() {
    let want: BTreeSet<&str> = [
        "rev-parse",
        "symbolic-ref",
        "check-ref-format",
        "merge-base",
        "status",
        "diff-tree",
        "diff-index",
        "diff-files",
        "ls-files",
        "merge-tree",
        "commit-tree",
        "read-tree",
        "update-index",
        "update-ref",
        "config",
    ]
    .into_iter()
    .collect();
    let have: BTreeSet<&str> = MERGE_AUDITED_COMMANDS.iter().copied().collect();
    assert_eq!(MERGE_AUDITED_COMMANDS.len(), 15);
    assert_eq!(have, want);
}

// ---- the docs tie ----

const DOC_SECTION: &str = "## Merge and push safety";
const ALLOWED_SECTION: &str = "### Allowed in merges";

fn doc_text() -> String {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../docs/reference/configuration.md");
    std::fs::read_to_string(&path).unwrap_or_else(|err| {
        panic!(
            "cannot read {}: {err} (the doc is owned by plan-113)",
            path.display()
        )
    })
}

/// The lines under a heading, up to the next heading of the same or a higher level.
fn section<'a>(doc: &'a str, heading: &str) -> Result<Vec<&'a str>, String> {
    let level = heading.chars().take_while(|c| *c == '#').count();
    let mut lines = doc.lines();
    if !lines.any(|line| line.trim_end() == heading) {
        return Err(format!(
            "docs/reference/configuration.md has no section '{heading}'; its format is owned by plan-113"
        ));
    }
    Ok(lines
        .take_while(|line| {
            let hashes = line.chars().take_while(|c| *c == '#').count();
            !(hashes > 0 && hashes <= level && line[hashes..].starts_with(' '))
        })
        .collect())
}

fn backticked(text: &str) -> Vec<&str> {
    text.split('`').skip(1).step_by(2).collect()
}

/// The (key, scope) rows of the table under the safety heading.
fn merge_table(doc: &str) -> Result<Vec<(String, String)>, String> {
    let bad = |what: &str, line: &str| {
        format!("'{DOC_SECTION}' table in configuration.md: {what} in row '{line}'; its format is owned by plan-113")
    };
    let lines = section(doc, DOC_SECTION)?;
    let mut rows = Vec::new();
    let mut header = false;
    for line in lines
        .iter()
        .filter(|line| line.trim_start().starts_with('|'))
    {
        let cells: Vec<&str> = line
            .trim()
            .trim_matches('|')
            .split('|')
            .map(str::trim)
            .collect();
        if cells.first() == Some(&"Key") {
            header = true;
            continue;
        }
        if cells
            .iter()
            .all(|cell| cell.chars().all(|c| c == '-' || c == ':'))
        {
            continue;
        }
        let key = cells
            .first()
            .and_then(|cell| cell.strip_prefix('`')?.strip_suffix('`'));
        let Some(key) = key else {
            return Err(bad("no backticked key", line));
        };
        let scope = cells.get(1).copied().unwrap_or("");
        if !["merge", "push", "both"].contains(&scope) {
            return Err(bad("no Scope cell of merge, push or both", line));
        }
        rows.push((key.to_string(), scope.to_string()));
    }
    if !header || rows.is_empty() {
        return Err(format!(
            "'{DOC_SECTION}' in configuration.md has no | Key | Scope | table; its format is owned by plan-113"
        ));
    }
    Ok(rows)
}

/// The backticked keys of the bullets under the allowed heading.
fn allowed_keys(doc: &str) -> Result<Vec<String>, String> {
    let keys: Vec<String> = section(doc, ALLOWED_SECTION)?
        .iter()
        .filter_map(|line| line.strip_prefix("- "))
        .flat_map(|bullet| {
            let head = bullet.split(':').next().unwrap_or("");
            backticked(head)
                .into_iter()
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect();
    if keys.is_empty() {
        return Err(format!(
            "'{ALLOWED_SECTION}' in configuration.md has no bullets with backticked keys; its format is owned by plan-113"
        ));
    }
    Ok(keys)
}

fn expected_scope(key: &str, merge_list: &[&str], doc_list: &[&str]) -> &'static str {
    match (merge_list.contains(&key), doc_list.contains(&key)) {
        (true, true) => "both",
        (true, false) => "merge",
        (false, _) => "push",
    }
}

#[test]
fn expected_scope_follows_the_two_lists() {
    let merge = ["a.b", "m.only", "both.key"];
    let doc = ["a.b", "both.key", "push.key"];
    assert_eq!(expected_scope("m.only", &merge, &doc), "merge");
    assert_eq!(expected_scope("both.key", &merge, &doc), "both");
    assert_eq!(expected_scope("push.key", &merge, &doc), "push");
}

#[test]
fn the_table_ties_to_the_code_lists() {
    let rows = merge_table(&doc_text()).unwrap();
    let merge_rows: BTreeSet<&str> = rows
        .iter()
        .filter(|(_, scope)| scope == "merge" || scope == "both")
        .map(|(key, _)| key.as_str())
        .collect();
    let merge_list: BTreeSet<&str> = MERGE_UNSAFE_FAMILIES.iter().copied().collect();
    assert_eq!(
        merge_rows, merge_list,
        "merge rows differ from MERGE_UNSAFE_FAMILIES"
    );
    for (key, scope) in &rows {
        assert_eq!(
            scope,
            expected_scope(key, MERGE_UNSAFE_FAMILIES, DOC_UNSAFE_FAMILIES),
            "scope of {key}"
        );
    }
}

fn sample_key(pattern: &str) -> String {
    pattern.replacen('*', "x", 1)
}

#[test]
fn every_allowed_key_is_clean_in_merge_scope() {
    for pattern in allowed_keys(&doc_text()).unwrap() {
        let key = sample_key(&pattern);
        let value = if key.eq_ignore_ascii_case("extensions.objectFormat") {
            "sha1"
        } else {
            "true"
        };
        let fx = fixture();
        if key.starts_with("extensions.") {
            git(
                &fx.root,
                &["config", "--local", "core.repositoryformatversion", "1"],
            );
        }
        git(&fx.root, &["config", "--local", &key, value]);
        assert_eq!(fx.check(UnsafeScope::Merge), UnsafeConfig::Clean, "{key}");
    }
}

#[test]
fn a_changed_doc_fails_with_a_message_naming_plan_113() {
    let missing = "# Config\n\n## Other\n\ntext\n";
    for err in [
        merge_table(missing).unwrap_err(),
        allowed_keys(missing).unwrap_err(),
    ] {
        assert!(err.contains("plan-113") && err.contains("'##"), "{err}");
    }
    let no_scope = format!("{DOC_SECTION}\n\n| Key | Scope |\n| --- | --- |\n| `a.b` |\n");
    let maybe = format!("{DOC_SECTION}\n\n| Key | Scope |\n| --- | --- |\n| `a.b` | maybe |\n");
    for doc in [no_scope, maybe] {
        let err = merge_table(&doc).unwrap_err();
        assert!(err.contains("plan-113") && err.contains("Scope"), "{err}");
    }
    let fine = format!("{DOC_SECTION}\n\n| Key | Scope |\n| --- | --- |\n| `a.b` | both |\n");
    assert_eq!(
        merge_table(&fine).unwrap(),
        [("a.b".to_string(), "both".to_string())]
    );
}
