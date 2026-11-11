//! The local knowledge base store on a temporary git repository and state directory (storage = local).
use std::path::Path;
use std::process::Command;

use capstan_config::kb::{parse_file, to_line, KbStorage};
use capstan_config::{Design, KbConfig, KbEntry, KbKind};
use capstan_daemon::kb::local::{self, Write};
use capstan_daemon::kb::{self, BriefKind};

const WHO: (&str, &str) = ("docs-1", "docs-keeper");

struct Repo {
    dir: tempfile::TempDir,
    state: tempfile::TempDir,
    cfg: KbConfig,
}

fn run(root: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn entry(id: &str, title: &str, path: Option<&str>, verified: &str) -> KbEntry {
    KbEntry {
        id: id.into(),
        kind: KbKind::Helper,
        title: title.into(),
        body: format!("body of {id}"),
        path: path.map(str::to_string),
        area: None,
        tags: vec![],
        aliases: vec![],
        related: vec![],
        verified_commit: verified.into(),
    }
}

impl Repo {
    fn new() -> Repo {
        let dir = tempfile::tempdir().unwrap();
        run(dir.path(), &["init", "-q", "-b", "main"]);
        let cfg = KbConfig {
            storage: KbStorage::Local,
            ..KbConfig::default()
        };
        let repo = Repo {
            dir,
            state: tempfile::tempdir().unwrap(),
            cfg,
        };
        repo.commit("a.txt", "a\n");
        repo
    }
    fn root(&self) -> &Path {
        self.dir.path()
    }
    fn state(&self) -> &Path {
        self.state.path()
    }
    fn commit(&self, rel: &str, text: &str) -> String {
        std::fs::write(self.root().join(rel), text).unwrap();
        run(self.root(), &["add", "-A"]);
        run(self.root(), &["commit", "-q", "-m", "chore: change"]);
        self.head()
    }
    fn head(&self) -> String {
        run(self.root(), &["rev-parse", "HEAD"])
    }
    fn file(&self) -> String {
        std::fs::read_to_string(self.state().join("kb/entries.jsonl")).unwrap_or_default()
    }
    fn write(&self, w: Write) -> Result<local::Change, String> {
        local::write(self.state(), self.root(), &self.cfg, w, WHO)
    }
    fn add(&self, e: KbEntry) -> Result<local::Change, String> {
        self.write(Write::Add {
            entry: e,
            force: false,
        })
    }
    fn status(&self) -> (usize, usize) {
        let s = kb::ensure(self.state(), self.root(), &self.cfg)
            .unwrap()
            .status()
            .unwrap();
        (s.stale, s.missing)
    }
}

#[test]
fn writes_touch_only_the_state_dir_and_the_index_follows() {
    let repo = Repo::new();
    let head = repo.head();
    repo.add(entry("fmt-bytes", "Format bytes", Some("a.txt"), &head))
        .unwrap();
    repo.add(entry("alpha", "Zebra stripes", None, &head))
        .unwrap();
    assert_eq!(run(repo.root(), &["status", "--porcelain"]), "");
    let ids: Vec<String> = parse_file(&repo.file(), 1200)
        .entries
        .into_iter()
        .map(|e| e.id)
        .collect();
    assert_eq!(ids, ["alpha", "fmt-bytes"]);
    let index = kb::ensure(repo.state(), repo.root(), &repo.cfg).unwrap();
    let hits = index.search(&["format".to_string()], 5).unwrap();
    assert_eq!(hits[0].id, "fmt-bytes");
    let mut changed = entry("fmt-bytes", "Render sizes", Some("a.txt"), &head);
    changed.body = "renders sizes".into();
    repo.write(Write::Update {
        entry: changed,
        expect: None,
    })
    .unwrap();
    let index = kb::ensure(repo.state(), repo.root(), &repo.cfg).unwrap();
    assert_eq!(
        index.search(&["render".to_string()], 5).unwrap()[0].id,
        "fmt-bytes"
    );
    repo.write(Write::Remove {
        id: "alpha".into(),
        expect: None,
    })
    .unwrap();
    assert_eq!(parse_file(&repo.file(), 1200).entries.len(), 1);
    assert_eq!(run(repo.root(), &["status", "--porcelain"]), "");
    assert!(repo.state().join("kb/entries.jsonl.prev").exists());
    assert!(repo.state().join("kb/changes.jsonl").exists());
    assert_eq!(
        local::entry(repo.state(), repo.root(), &repo.cfg, "fmt-bytes")
            .unwrap()
            .title,
        "Render sizes"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(repo.state().join("kb/entries.jsonl"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}

#[test]
fn refusals() {
    let mut repo = Repo::new();
    let head = repo.head();
    let place = kb::location(repo.state(), repo.root(), &repo.cfg);
    repo.add(entry("one", "Format bytes", Some("a.txt"), &head))
        .unwrap();
    assert_eq!(
        repo.add(entry("one", "Other thing", None, &head))
            .unwrap_err(),
        "kb: id one exists; use cstan kb update one"
    );
    assert_eq!(
        repo.add(entry("two", "Format bytes", Some("a.txt"), &head))
            .unwrap_err(),
        "kb: similar entries exist: one; reuse or update one, or add --force"
    );
    repo.write(Write::Add {
        entry: entry("two", "Format bytes", Some("a.txt"), &head),
        force: true,
    })
    .unwrap();
    assert_eq!(
        repo.write(Write::Update {
            entry: entry("nope", "T", None, &head),
            expect: None
        })
        .unwrap_err(),
        format!("kb: no entry nope in {place}")
    );
    assert_eq!(
        repo.write(Write::Remove {
            id: "nope".into(),
            expect: None
        })
        .unwrap_err(),
        format!("kb: no entry nope in {place}")
    );
    let bad = "1234567890abcdef1234567890abcdef12345678";
    assert_eq!(
        repo.add(entry("three", "Unique words here", None, bad))
            .unwrap_err(),
        "kb: verified commit 1234567890ab is not in the project repository"
    );
    repo.cfg.max_entries = 2;
    assert_eq!(
        repo.add(entry("four", "Quite different", None, &head))
            .unwrap_err(),
        format!("kb: {place} already has 2 entries, the limit of 2; remove one first")
    );
    assert!(place.ends_with("kb/entries.jsonl"), "{place}");
}

#[test]
fn concurrent_writers_lose_nothing() {
    let repo = Repo::new();
    let head = repo.head();
    std::thread::scope(|s| {
        for t in 0..2 {
            let (repo, head) = (&repo, head.clone());
            s.spawn(move || {
                for i in 0..50 {
                    let id = format!("e-{t}-{i:02}");
                    let e = entry(&id, &format!("unique{t}x{i} thing"), None, &head);
                    repo.write(Write::Add {
                        entry: e,
                        force: true,
                    })
                    .unwrap();
                }
            });
        }
    });
    let parsed = parse_file(&repo.file(), 1200);
    assert!(parsed.invalid.is_empty());
    let ids: Vec<String> = parsed.entries.iter().map(|e| e.id.clone()).collect();
    assert_eq!(ids.len(), 100);
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(ids, sorted);
}

#[test]
fn a_left_over_temp_file_does_not_corrupt_and_prev_is_the_previous_content() {
    let repo = Repo::new();
    let head = repo.head();
    repo.add(entry("one", "First thing", None, &head)).unwrap();
    let before = repo.file();
    std::fs::write(repo.state().join("kb/entries.jsonl.tmp"), "{ torn").unwrap();
    assert_eq!(repo.file(), before);
    repo.add(entry("two", "Second thing", None, &head)).unwrap();
    let prev = std::fs::read_to_string(repo.state().join("kb/entries.jsonl.prev")).unwrap();
    assert_eq!(prev, before);
    let parsed = parse_file(&repo.file(), 1200);
    assert!(parsed.invalid.is_empty());
    assert_eq!(parsed.entries.len(), 2);
}

#[test]
fn the_change_log_is_trimmed_and_newest_first() {
    let mut repo = Repo::new();
    repo.cfg.log_max_entries = 3;
    let head = repo.head();
    for i in 0..5 {
        repo.add(entry(
            &format!("e{i}"),
            &format!("distinct{i}word"),
            None,
            &head,
        ))
        .unwrap();
    }
    let text = std::fs::read_to_string(repo.state().join("kb/changes.jsonl")).unwrap();
    assert_eq!(text.lines().count(), 3);
    let log = local::log(repo.state(), 10);
    assert_eq!(
        log.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
        ["e4", "e3", "e2"]
    );
    assert_eq!(
        (
            log[0].agent.as_str(),
            log[0].role.as_str(),
            log[0].verb.as_str()
        ),
        ("docs-1", "docs-keeper", "add")
    );
    assert_eq!(local::log(repo.state(), 1).len(), 1);
}

#[test]
fn import_adds_skips_reports_invalid_and_replaces() {
    let mut repo = Repo::new();
    let head = repo.head();
    repo.add(entry("old", "Existing thing", None, &head))
        .unwrap();
    let mut text = String::new();
    for (id, title) in [
        ("n1", "alpha one"),
        ("n2", "beta two"),
        ("n3", "gamma three"),
    ] {
        text.push_str(&to_line(&entry(id, title, None, &head)));
        text.push('\n');
    }
    text.push_str("not json\n");
    let mut changed = entry("old", "Replaced title", None, &head);
    changed.body = "new".into();
    text.push_str(&to_line(&changed));
    text.push('\n');
    let r = local::import(repo.state(), repo.root(), &repo.cfg, &text, false, WHO).unwrap();
    assert_eq!((r.added, r.replaced, r.skipped), (3, 0, 1));
    assert_eq!(r.invalid.len(), 1);
    assert_eq!(r.invalid[0].0, 4);
    assert_eq!(
        local::entry(repo.state(), repo.root(), &repo.cfg, "old")
            .unwrap()
            .title,
        "Existing thing"
    );
    assert_eq!(local::log(repo.state(), 1)[0].verb, "import");
    assert_eq!(local::log(repo.state(), 1)[0].count, 3);
    let r = local::import(repo.state(), repo.root(), &repo.cfg, &text, true, WHO).unwrap();
    assert_eq!((r.added, r.replaced, r.skipped), (0, 4, 0));
    assert_eq!(
        local::entry(repo.state(), repo.root(), &repo.cfg, "old")
            .unwrap()
            .title,
        "Replaced title"
    );
    let before = repo.file();
    repo.cfg.max_entries = 4;
    let more = to_line(&entry("zz", "extra", None, &head));
    let err = local::import(repo.state(), repo.root(), &repo.cfg, &more, false, WHO).unwrap_err();
    assert!(err.contains("over the limit of 4"), "{err}");
    assert_eq!(repo.file(), before);
}

#[test]
fn staleness_matches_repo_mode() {
    let repo = Repo::new();
    let head = repo.head();
    repo.add(entry("one", "Tracked file", Some("a.txt"), &head))
        .unwrap();
    repo.add(entry("two", "Gone file", Some("gone.txt"), &head))
        .unwrap();
    assert_eq!(repo.status(), (0, 1));
    repo.commit("a.txt", "changed\n");
    assert_eq!(repo.status(), (1, 1));
}

#[test]
fn brief_and_leftover_warning_in_local_mode() {
    let repo = Repo::new();
    let head = repo.head();
    repo.add(entry("one", "Tracked file", Some("a.txt"), &head))
        .unwrap();
    let index = kb::ensure(repo.state(), repo.root(), &repo.cfg).unwrap();
    let text = kb::brief(&index, &repo.cfg, BriefKind::Build, false);
    assert!(text.contains("local knowledge base at"), "{text}");
    assert!(
        text.contains("Commit no knowledge base file; report when done."),
        "{text}"
    );
    assert!(!text.contains("on your branch"), "{text}");
    let warnings = kb::warnings(
        repo.state(),
        repo.root(),
        &repo.cfg,
        None,
        &Design::default(),
    );
    assert!(
        warnings.iter().all(|w| !w.contains("storage is local")),
        "{warnings:?}"
    );
    std::fs::create_dir_all(repo.root().join("docs/capstan")).unwrap();
    repo.commit("docs/capstan/kb.jsonl", "");
    let warnings = kb::warnings(
        repo.state(),
        repo.root(),
        &repo.cfg,
        None,
        &Design::default(),
    );
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("is in the repository but [kb] storage is local")),
        "{warnings:?}"
    );
}

#[test]
fn a_log_failure_does_not_refuse_the_write() {
    let repo = Repo::new();
    let head = repo.head();
    // A directory where the change log should be makes every log read fail with a non-NotFound error.
    std::fs::create_dir_all(repo.state().join("kb/changes.jsonl")).unwrap();
    let change = repo
        .add(entry("alpha", "Alpha helper", None, &head))
        .expect("the write succeeds");
    assert_eq!(change.id, "alpha");
    assert!(repo.file().contains("\"alpha\""));
}

#[test]
fn add_checks_similar_entries_against_the_file_under_the_lock() {
    let repo = Repo::new();
    let head = repo.head();
    repo.add(entry("fmt-bytes", "Format bytes", Some("a.txt"), &head))
        .unwrap();
    let again = local::write(
        repo.state(),
        repo.root(),
        &repo.cfg,
        Write::Add {
            entry: entry("fmt-bytes-2", "Format bytes", Some("a.txt"), &head),
            force: false,
        },
        WHO,
    );
    assert!(again.unwrap_err().contains("similar entries exist"));
}

#[test]
fn an_empty_entries_file_is_a_missing_knowledge_base() {
    let repo = Repo::new();
    std::fs::create_dir_all(repo.state().join("kb")).unwrap();
    std::fs::write(repo.state().join("kb/entries.jsonl"), "\n").unwrap();
    let s = kb::ensure(repo.state(), repo.root(), &repo.cfg)
        .unwrap()
        .status()
        .unwrap();
    assert_eq!(s.state, "missing");
}

#[test]
fn a_stale_expect_refuses_update_and_remove_but_the_current_sha_passes() {
    let repo = Repo::new();
    let head = repo.head();
    repo.add(entry("alpha", "Alpha entry", None, &head))
        .unwrap();
    let old = local::line_sha(&entry("alpha", "Alpha entry", None, &head));
    let newer = entry("alpha", "Alpha entry v2", None, &head);
    repo.write(Write::Update {
        entry: newer.clone(),
        expect: Some(old.clone()),
    })
    .unwrap();
    let why = repo
        .write(Write::Update {
            entry: entry("alpha", "Alpha v3", None, &head),
            expect: Some(old.clone()),
        })
        .unwrap_err();
    assert_eq!(
        why,
        "kb: alpha changed since it was read; run the command again."
    );
    assert!(repo
        .write(Write::Remove {
            id: "alpha".into(),
            expect: Some(old)
        })
        .unwrap_err()
        .contains("changed since it was read"));
    repo.write(Write::Remove {
        id: "alpha".into(),
        expect: Some(local::line_sha(&newer)),
    })
    .unwrap();
}
