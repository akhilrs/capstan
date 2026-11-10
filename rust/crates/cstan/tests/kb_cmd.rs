//! `cstan kb ...` end to end: the real binary (which embeds the daemon) on a scratch git project whose `[kb]` entries file
//! is committed. The lookups read the file as committed at HEAD; add, update and remove rewrite the working tree's file.
mod support;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{json, Value};

const OPERATOR: &str = "operator-0123456789abcdef0123456789abcdef0123456789";
const FILE: &str = "docs/capstan/kb.jsonl";

struct Project {
    base: PathBuf,
    root: PathBuf,
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = Command::new(env!("CARGO_BIN_EXE_cstan"))
            .arg("stop")
            .current_dir(&self.root)
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("CAPSTAN_TRUST_DIR", support::trust_dir_of(&self.root))
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

fn line(
    id: &str,
    kind: &str,
    title: &str,
    body: &str,
    path: Option<&str>,
    verified: &str,
) -> String {
    let mut entry = json!({"id": id, "kind": kind, "title": title, "body": body});
    if let Some(path) = path {
        entry["path"] = json!(path);
    }
    entry["verified_commit"] = json!(verified);
    entry.to_string()
}

/// `kb` is the text of the `[kb]` section (`None` leaves it out); `entries` is committed as the entries file.
fn project(label: &str, kb: Option<&str>, entries: &[&str]) -> Project {
    let base = capstan_testkit::short_temp_root()
        .join(format!("capstan-kbcmd-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let root = base.join("proj");
    std::fs::create_dir_all(root.join("src")).unwrap();
    git(&root, &["init", "--quiet", "--initial-branch=main"]);
    std::fs::write(root.join("src/util.rs"), "fn a() {}\n").unwrap();
    std::fs::write(root.join(".git/info/exclude"), ".capstan/\n").unwrap();
    git(&root, &["add", "--all"]);
    git(&root, &["commit", "--quiet", "-m", "chore: initial commit"]);
    let head = git(&root, &["rev-parse", "HEAD"]);
    if !entries.is_empty() {
        std::fs::create_dir_all(root.join("docs/capstan")).unwrap();
        let text: String = entries
            .iter()
            .map(|e| format!("{}\n", e.replace("HEADSHA", &head)))
            .collect();
        std::fs::write(root.join(FILE), text).unwrap();
        git(&root, &["add", "--all"]);
        git(
            &root,
            &["commit", "--quiet", "-m", "docs: add the knowledge base"],
        );
    }
    let capstan = root.join(".capstan");
    let state = capstan.join("state");
    std::fs::create_dir_all(&state).unwrap();
    private(&capstan, 0o700);
    private(&state, 0o700);
    let config = json!({
        "schemaVersion": 1, "projectId": "pkbcmd", "name": "Kb",
        "stateDirectory": state.to_string_lossy(),
        "maxSlices": 4, "maxRunMs": 3_600_000, "maxDispatches": 16,
    });
    std::fs::write(root.join(cstan_front::projects::UNDISCOVERABLE_MARKER), "").unwrap();
    for (name, text) in [
        ("project.json", format!("{config:#}\n")),
        ("operator.key", format!("{OPERATOR}\n")),
    ] {
        std::fs::write(capstan.join(name), text).unwrap();
        private(&capstan.join(name), 0o600);
    }
    let toml = format!(
        "schema_version = 1\n[nexora]\ntrack = \"never\"\n[hosts.claude]\nkind = \"claude\"\n\
         [roles.pm]\nkind = \"PM\"\nhost = \"claude\"\n[roles.developer]\nkind = \"Developer\"\nhost = \"claude\"\n{}\n",
        kb.map(|k| format!("[kb]\n{k}")).unwrap_or_default()
    );
    std::fs::write(root.join("capstan.toml"), toml).unwrap();
    private(&root.join("capstan.toml"), 0o600);
    support::trust_beside(&root);
    Project { base, root }
}

impl Project {
    fn cstan(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_cstan"))
            .args(args)
            .current_dir(&self.root)
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("CAPSTAN_TRUST_DIR", support::trust_dir_of(&self.root))
            .env("HOME", "/nonexistent")
            .stdin(Stdio::null())
            .output()
            .expect("cstan runs")
    }

    /// (exit code, stdout, stderr)
    fn run(&self, args: &[&str]) -> (i32, String, String) {
        let o = self.cstan(args);
        (
            o.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&o.stdout).into_owned(),
            String::from_utf8_lossy(&o.stderr).into_owned(),
        )
    }

    fn file(&self) -> String {
        std::fs::read_to_string(self.root.join(FILE)).unwrap_or_default()
    }

    fn head(&self) -> String {
        git(&self.root, &["rev-parse", "HEAD"])
    }
}

fn seed() -> Vec<String> {
    vec![
        line(
            "fmt-bytes",
            "helper",
            "Format a byte count",
            "Turns 1536 into 1.5 KiB for every table.",
            Some("src/util.rs"),
            "HEADSHA",
        ),
        line(
            "retry-policy",
            "convention",
            "Retry policy",
            "Every outbound call retries with backoff.",
            None,
            "HEADSHA",
        ),
    ]
}

#[test]
fn search_for_uses_and_status_print_lines_and_json() {
    let seeded = seed();
    let refs: Vec<&str> = seeded.iter().map(String::as_str).collect();
    let p = project("read", Some(""), &refs);
    let (code, out, _) = p.run(&["kb", "search", "bytes"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.starts_with("fmt-bytes  helper  Format a byte count  src/util.rs\n"),
        "{out}"
    );
    assert!(out.contains("\n    Turns 1536 into 1.5 KiB"), "{out}");
    let (_, json, _) = p.run(&["kb", "search", "retry", "--limit", "1", "--json"]);
    let hits: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(hits[0]["id"], "retry-policy");
    assert_eq!(hits.as_array().unwrap().len(), 1);
    let (code, out, _) = p.run(&["kb", "search", "zzzzqqq"]);
    assert_eq!((code, out.as_str()), (0, "kb: no entries\n"));
    let (_, out, _) = p.run(&["kb", "for", "src"]);
    assert!(out.starts_with("fmt-bytes  helper"), "{out}");
    let (code, out, _) = p.run(&["kb", "uses", "fmt-bytes"]);
    assert_eq!((code, out.as_str()), (0, "kb: no entries\n"));
    let (_, out, _) = p.run(&["kb", "status"]);
    assert!(out.starts_with("kb: ok  head "), "{out}");
    assert!(out.contains("entries 2"), "{out}");
    let (_, json, _) = p.run(&["kb", "status", "--json"]);
    let status: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(status["entries"], 2);
}

#[test]
fn build_and_refresh_print_the_brief_and_the_next_step() {
    let p = project("brief", Some(""), &[]);
    let (code, out, err) = p.run(&["kb", "build"]);
    assert_eq!(code, 0, "{err}");
    assert!(
        out.contains("Build the knowledge base at docs/capstan/kb.jsonl"),
        "{out}"
    );
    assert!(
        err.contains("next: cstan spawn docs-keeper --title \"knowledge base\" then cstan send <agent-id> with this brief"),
        "{err}"
    );
    let (_, out, err) = p.run(&["kb", "refresh", "--design"]);
    assert!(out.contains("design record"), "{out}");
    assert!(err.contains("next: cstan spawn designer"), "{err}");
}

#[test]
fn without_a_kb_table_every_form_exits_1() {
    let p = project("off", None, &[]);
    for args in [
        vec!["kb", "search", "x"],
        vec!["kb", "status"],
        vec!["kb", "build"],
        vec![
            "kb", "add", "--id", "a", "--kind", "helper", "--title", "t", "--body", "b",
        ],
        vec!["kb", "remove", "a"],
    ] {
        let (code, out, err) = p.run(&args);
        assert_eq!(code, 1, "{args:?}: {out}{err}");
        assert_eq!(
            err, "kb: not configured; add a [kb] table to capstan.toml\n",
            "{args:?}"
        );
    }
}

#[test]
fn add_writes_a_sorted_line_stamped_with_head_and_keeps_the_others() {
    let seeded = seed();
    let refs: Vec<&str> = seeded.iter().map(String::as_str).collect();
    let p = project("add", Some(""), &refs);
    let before = p.file();
    let (code, out, err) = p.run(&[
        "kb",
        "add",
        "--id",
        "dates",
        "--kind",
        "helper",
        "--title",
        "Parse a calendar date",
        "--body",
        "Accepts ISO dates only.",
        "--path",
        "src/util.rs",
        "--tag",
        "time",
        "--alias",
        "iso",
        "--force",
    ]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        out,
        format!("kb: added dates to {FILE}; commit it on your branch\n")
    );
    let after = p.file();
    let lines: Vec<&str> = after.lines().collect();
    assert_eq!(lines.len(), 3);
    assert!(lines[0].contains("\"id\":\"dates\""), "{after}");
    assert!(lines[0].contains(&format!("\"verified_commit\":\"{}\"", p.head())));
    let old: Vec<&str> = before.lines().collect();
    assert_eq!(&lines[1..], &old[..]);
    // the same id again
    let (code, _, err) = p.run(&[
        "kb",
        "add",
        "--id",
        "fmt-bytes",
        "--kind",
        "helper",
        "--title",
        "Other",
        "--body",
        "x",
    ]);
    assert_eq!(code, 1);
    assert!(
        err.starts_with("kb: id fmt-bytes exists; use cstan kb update fmt-bytes"),
        "{err}"
    );
    // an id only in the working tree's file is refused by the command itself
    let (code, _, err) = p.run(&[
        "kb",
        "add",
        "--id",
        "dates",
        "--kind",
        "helper",
        "--title",
        "Zzz other",
        "--body",
        "x",
    ]);
    assert_eq!(code, 1);
    assert!(err.starts_with("kb: id dates is already in"), "{err}");
}

#[test]
fn add_refuses_a_field_rule_a_similar_entry_and_a_full_file() {
    let seeded = seed();
    let refs: Vec<&str> = seeded.iter().map(String::as_str).collect();
    let p = project("refuse", Some("max_entries = 2\n"), &refs);
    let (code, _, err) = p.run(&[
        "kb", "add", "--id", "Bad Id", "--kind", "helper", "--title", "t", "--body", "b",
    ]);
    assert_eq!(code, 1);
    assert!(err.starts_with("kb entry: id must match"), "{err}");
    let (code, _, err) = p.run(&[
        "kb",
        "add",
        "--id",
        "fmt-size",
        "--kind",
        "helper",
        "--title",
        "Format a byte count",
        "--body",
        "b",
    ]);
    assert_eq!(code, 1);
    assert!(
        err.starts_with(
            "kb: similar entries exist: fmt-bytes; reuse or update one, or add --force"
        ),
        "{err}"
    );
    let (code, _, err) = p.run(&[
        "kb", "add", "--id", "fresh", "--kind", "decision", "--title", "Zebra", "--body", "b",
    ]);
    assert_eq!(code, 1);
    assert!(err.contains("limit of 2"), "{err}");
    assert_eq!(p.file().lines().count(), 2);
}

#[test]
fn update_changes_only_the_named_fields_and_verified_only_re_stamps() {
    let seeded = seed();
    let refs: Vec<&str> = seeded.iter().map(String::as_str).collect();
    let p = project("update", Some(""), &refs);
    std::fs::write(p.root.join("src/more.rs"), "fn b() {}\n").unwrap();
    git(&p.root, &["add", "--all"]);
    git(&p.root, &["commit", "--quiet", "-m", "feat: more"]);
    let head = p.head();
    let (code, out, err) = p.run(&[
        "kb",
        "update",
        "retry-policy",
        "--body",
        "Retries three times.",
    ]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        out,
        format!("kb: updated retry-policy in {FILE}; commit it on your branch\n")
    );
    let text = p.file();
    let updated: Vec<Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(updated[1]["body"], "Retries three times.");
    assert_eq!(updated[1]["title"], "Retry policy");
    assert_eq!(updated[1]["verified_commit"], head.as_str());
    assert_eq!(
        text.lines().next().unwrap(),
        seeded[0].replace("HEADSHA", &git(&p.root, &["rev-parse", "HEAD~2"]))
    );
    let (code, _, err) = p.run(&["kb", "update", "fmt-bytes", "--verified"]);
    assert_eq!(code, 0, "{err}");
    let text = p.file();
    let first: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(first["verified_commit"], head.as_str());
    assert_eq!(first["title"], "Format a byte count");
    let (code, _, err) = p.run(&["kb", "update", "nothing", "--verified"]);
    assert_eq!(code, 1);
    assert!(err.starts_with("kb: no entry nothing in "), "{err}");
}

#[test]
fn remove_deletes_the_line_and_refuses_an_absent_id() {
    let seeded = seed();
    let refs: Vec<&str> = seeded.iter().map(String::as_str).collect();
    let p = project("remove", Some(""), &refs);
    let (code, out, err) = p.run(&["kb", "remove", "fmt-bytes"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        out,
        format!("kb: removed fmt-bytes from {FILE}; commit it on your branch\n")
    );
    assert_eq!(p.file().lines().count(), 1);
    let (code, _, err) = p.run(&["kb", "remove", "fmt-bytes"]);
    assert_eq!(code, 1);
    assert!(err.starts_with("kb: no entry fmt-bytes in "), "{err}");
    assert!(git(&p.root, &["status", "--porcelain"]).contains("kb.jsonl"));
}

#[test]
fn force_and_verified_are_refused_outside_their_verb() {
    let seeded = seed();
    let refs: Vec<&str> = seeded.iter().map(String::as_str).collect();
    let p = project("flags", Some(""), &refs);
    let before = p.file();
    let add = [
        "kb", "add", "--id", "fresh", "--kind", "helper", "--title", "t", "--body", "b",
    ];
    let mut with_verified = add.to_vec();
    with_verified.push("--verified");
    let update_force = ["kb", "update", "fmt-bytes", "--force"];
    let remove_force = ["kb", "remove", "fmt-bytes", "--force"];
    let update_verified_force = ["kb", "update", "fmt-bytes", "--verified", "--force"];
    for args in [
        &with_verified[..],
        &update_force[..],
        &remove_force[..],
        &update_verified_force[..],
    ] {
        let (code, _, err) = p.run(args);
        assert_ne!(code, 0, "{args:?}");
        assert!(
            err.contains("usage") || err.contains("Usage"),
            "{args:?}: {err}"
        );
    }
    assert_eq!(p.file(), before);
}
