//! The knowledge base index on a temporary git repository and state directory.
use std::path::{Path, PathBuf};
use std::process::Command;

use capstan_config::kb::to_line;
use capstan_config::{Design, KbConfig, KbEntry, KbKind};
use capstan_daemon::kb::{self, BriefKind, KbError};
use rusqlite::Connection;

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
        let repo = Repo {
            dir,
            state: tempfile::tempdir().unwrap(),
            cfg: KbConfig::default(),
        };
        repo.write("README.md", "hello\n");
        repo.commit("chore: start");
        repo
    }
    fn root(&self) -> &Path {
        self.dir.path()
    }
    fn write(&self, rel: &str, text: &str) {
        let p = self.root().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    fn commit(&self, msg: &str) -> String {
        run(self.root(), &["add", "-A"]);
        run(self.root(), &["commit", "-q", "-m", msg]);
        self.head()
    }
    fn head(&self) -> String {
        run(self.root(), &["rev-parse", "HEAD"])
    }
    fn put(&self, entries: &[KbEntry]) {
        let text: String = entries
            .iter()
            .map(|e| format!("{}\n", to_line(e)))
            .collect();
        self.write(&self.cfg.path, &text);
    }
    fn ensure(&self) -> kb::KbIndex {
        kb::ensure(self.state.path(), self.root(), &self.cfg).unwrap()
    }
    fn db(&self) -> PathBuf {
        self.state.path().join("kb/index.sqlite")
    }
}

fn words(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_string).collect()
}

fn ids(hits: &[kb::Hit]) -> Vec<&str> {
    hits.iter().map(|h| h.id.as_str()).collect()
}

#[test]
fn fts5_is_available() {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch(
        "CREATE VIRTUAL TABLE t USING fts5(a, tokenize = 'unicode61 remove_diacritics 2')",
    )
    .unwrap();
}

#[test]
fn not_configured_and_missing_file() {
    let mut repo = Repo::new();
    repo.cfg.enabled = false;
    assert_eq!(
        kb::ensure(repo.state.path(), repo.root(), &repo.cfg).err(),
        Some(KbError::NotConfigured)
    );
    repo.cfg.enabled = true;
    let index = repo.ensure();
    let status = index.status().unwrap();
    assert_eq!((status.state.as_str(), status.entries), ("missing", 0));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(repo.state.path().join("kb"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
    }
}

#[test]
fn build_skip_and_rebuild_keep_queries() {
    let repo = Repo::new();
    let base = repo.head();
    repo.put(&[entry(
        "http_client",
        "HttpClient wrapper",
        Some("src/http.rs"),
        &base,
    )]);
    // an uncommitted file is not read
    let index = repo.ensure();
    assert_eq!(index.status().unwrap().state, "missing");
    repo.commit("docs: kb");
    let index = repo.ensure();
    assert_eq!(index.status().unwrap().entries, 1);
    index.search(&words("http"), 5).unwrap();
    drop(index);
    let conn = Connection::open(repo.db()).unwrap();
    conn.execute("INSERT INTO meta(key, value) VALUES('marker', 'x')", [])
        .unwrap();
    drop(conn);
    repo.ensure();
    let conn = Connection::open(repo.db()).unwrap();
    let marker: String = conn
        .query_row("SELECT value FROM meta WHERE key='marker'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(marker, "x");
    drop(conn);
    // a new commit of the file rebuilds
    repo.put(&[
        entry(
            "http_client",
            "HttpClient wrapper",
            Some("src/http.rs"),
            &base,
        ),
        entry("other", "Other thing", None, &base),
    ]);
    repo.commit("docs: kb again");
    let index = repo.ensure();
    let status = index.status().unwrap();
    assert_eq!((status.entries, status.searches_30d), (2, 1));
    assert_eq!(status.head, repo.head());
}

#[test]
fn search_matches_and_ranks() {
    let repo = Repo::new();
    let v = repo.head();
    let mut a = entry("retry_backoff", "Backoff schedule", None, &v);
    a.tags = vec!["resilience".into()];
    let mut b = entry("zeta", "Zeta", None, &v);
    b.body = "this mentions the server in the body wiring".into();
    let c = entry("alpha", "HttpServer setup", None, &v);
    repo.put(&[a, b, c]);
    repo.commit("docs: kb");
    let index = repo.ensure();
    assert_eq!(
        ids(&index.search(&words("backoff"), 5).unwrap()),
        ["retry_backoff"]
    );
    assert_eq!(
        ids(&index.search(&words("retry"), 5).unwrap()),
        ["retry_backoff"]
    );
    assert_eq!(
        ids(&index.search(&words("resil"), 5).unwrap()),
        ["retry_backoff"]
    );
    let hits = index.search(&words("server"), 5).unwrap();
    assert_eq!(ids(&hits), ["alpha", "zeta"]);
    assert_eq!(
        ids(&index.search(&words("HttpServer"), 5).unwrap())[0],
        "alpha"
    );
    assert!(index
        .search(&words("nothing-here \"quoted"), 5)
        .unwrap()
        .is_empty());
}

#[test]
fn for_paths_and_uses() {
    let repo = Repo::new();
    let v = repo.head();
    let mut user = entry("page", "A page", Some("src/page.rs"), &v);
    user.related = vec!["path:src/ui/button.rs".into(), "button".into()];
    let mut deep = entry("screen", "A screen", None, &v);
    deep.related = vec!["page".into()];
    let mut top = entry("shell", "Shell", None, &v);
    top.related = vec!["screen".into()];
    repo.put(&[
        entry("button", "Button", Some("src/ui/button.rs"), &v),
        entry("ui_dir", "UI dir", Some("src/ui"), &v),
        entry("zz_other", "Other", Some("lib/other.rs"), &v),
        user,
        deep,
        top,
    ]);
    repo.commit("docs: kb");
    let index = repo.ensure();
    let file = index.for_paths(&["src/ui/button.rs".to_string()]).unwrap();
    assert_eq!(ids(&file), ["ui_dir", "button", "page"]);
    assert!(file[2].uses && !file[0].uses);
    let dir = index.for_paths(&["src/ui".to_string()]).unwrap();
    assert_eq!(ids(&dir), ["ui_dir", "button"]);
    let glob = index
        .for_paths(&kb::owned_paths(&["src/ui/** extra words".to_string()]))
        .unwrap();
    assert_eq!(ids(&glob), ["ui_dir", "button"]);
    assert_eq!(
        ids(&index.uses("button").unwrap()),
        ["page", "screen", "shell"]
    );
    assert_eq!(
        ids(&index.uses("path:src/ui/button.rs").unwrap()),
        ["page", "screen", "shell"]
    );
    assert_eq!(
        ids(&index.uses("src/ui/button.rs").unwrap()),
        ["page", "screen", "shell"]
    );
    assert!(index.uses("shell").unwrap().is_empty());
}

#[test]
fn staleness() {
    let repo = Repo::new();
    repo.write("src/a.rs", "1\n");
    repo.write("src/b.rs", "1\n");
    repo.write("src/dir/c.rs", "1\n");
    let v = repo.commit("feat: sources");
    repo.write("src/a.rs", "2\n");
    repo.write("src/dir/c.rs", "2\n");
    repo.commit("fix: change a and c");
    let unknown = "0123456789abcdef0123456789abcdef01234567";
    repo.put(&[
        entry("a", "A", Some("src/a.rs"), &v),
        entry("b", "B", Some("src/b.rs"), &v),
        entry("c", "C", Some("src/dir"), &v),
        entry("gone", "Gone", Some("src/gone.rs"), &v),
        entry("nopath", "No path", None, unknown),
        entry("unk", "Unknown", Some("src/b.rs"), unknown),
    ]);
    repo.commit("docs: kb");
    let index = repo.ensure();
    let all = index.for_paths(&["src".to_string()]).unwrap();
    let get = |id: &str| all.iter().find(|h| h.id == id).unwrap().clone();
    assert_eq!(get("a").state, "stale");
    assert_eq!(get("a").reason, format!("changed since {}", &v[..12]));
    assert_eq!(get("b").state, "ok");
    assert_eq!(get("c").state, "stale");
    assert_eq!(get("unk").reason, "verified commit unknown");
    // the missing entry has no file under src/gone.rs, so it is found by a search
    let gone = index.search(&words("gone"), 3).unwrap();
    assert_eq!(
        (gone[0].state.as_str(), gone[0].reason.as_str()),
        ("missing", "not at HEAD")
    );
    let status = index.status().unwrap();
    assert_eq!((status.stale, status.missing), (3, 1));
    // deleting a file makes its entry missing
    run(repo.root(), &["rm", "-q", "src/b.rs"]);
    repo.commit("refactor: drop b");
    let index = repo.ensure();
    assert_eq!(index.status().unwrap().missing, 3);
}

#[test]
fn status_invalid_and_vectors_hint() {
    let repo = Repo::new();
    let v = repo.head();
    repo.put(&[entry("one", "One", None, &v)]);
    let mut text = std::fs::read_to_string(repo.root().join(&repo.cfg.path)).unwrap();
    text.push_str("not json\n");
    repo.write(&repo.cfg.path, &text);
    repo.commit("docs: kb");
    let index = repo.ensure();
    let status = index.status().unwrap();
    assert_eq!(status.invalid.len(), 1);
    assert_eq!(status.invalid[0].0, 2);
    assert_eq!(status.vectors_hint, "not needed");
    for _ in 0..40 {
        index.search(&words("one"), 5).unwrap();
    }
    for _ in 0..9 {
        index.search(&words("zzz"), 5).unwrap();
    }
    assert_eq!(index.status().unwrap().vectors_hint, "not needed");
    index.search(&words("zzz"), 5).unwrap();
    let status = index.status().unwrap();
    assert_eq!((status.searches_30d, status.misses_30d), (50, 10));
    assert_eq!(status.vectors_hint, "consider");
}

#[test]
fn similar_entries() {
    let repo = Repo::new();
    let v = repo.head();
    repo.put(&[
        entry("one", "Retry helper", Some("src/retry.rs"), &v),
        entry("two", "Another one", Some("src/retry.rs"), &v),
        entry("three", "Retry helper for http", None, &v),
        entry("four", "Unrelated", None, &v),
    ]);
    repo.commit("docs: kb");
    let index = repo.ensure();
    let probe = entry("new", "retry helper", Some("src/retry.rs"), &v);
    assert_eq!(index.similar(&probe).unwrap(), ["one", "two", "three"]);
}

#[test]
fn reuse_lines_order_markers_and_cut() {
    let repo = Repo::new();
    repo.write("src/a.rs", "1\n");
    let v = repo.commit("feat: a");
    repo.write("src/a.rs", "2\n");
    repo.commit("fix: a");
    let mut area = entry("area_one", "Area entry", Some("src/b.rs"), &v);
    area.area = Some("core".into());
    repo.put(&[entry("a", "Alpha", Some("src/a.rs"), &v), area]);
    repo.commit("docs: kb");
    let index = repo.ensure();
    let owns = vec!["src/**".to_string()];
    let text = kb::reuse_lines(&index, &owns, 1500).unwrap();
    assert_eq!(
        text,
        "- a (helper) Alpha: src/a.rs [stale]\n- area_one (helper) Area entry: src/b.rs [missing]"
    );
    let cut = kb::reuse_lines(&index, &owns, 70).unwrap();
    assert_eq!(
        cut,
        "- a (helper) Alpha: src/a.rs [stale]\n- (more: cstan kb for src)"
    );
    assert_eq!(kb::reuse_lines(&index, &owns, 0), None);
    assert_eq!(kb::reuse_lines(&index, &["docs/x".to_string()], 1500), None);
}

#[test]
fn warning_texts() {
    let mut repo = Repo::new();
    let design = Design::default();
    let path = repo.cfg.path.clone();
    let w = kb::warnings(repo.state.path(), repo.root(), &repo.cfg, None, &design);
    assert_eq!(w, [format!("kb: no knowledge base at {path}; next: cstan kb build and send its brief to a docs-keeper agent")]);
    repo.write("src/a.rs", "1\n");
    let v = repo.commit("feat: a");
    repo.write("src/a.rs", "2\n");
    repo.commit("fix: a");
    repo.put(&[
        entry("a", "A", Some("src/a.rs"), &v),
        entry("gone", "G", Some("src/gone.rs"), &v),
    ]);
    let mut text = std::fs::read_to_string(repo.root().join(&path)).unwrap();
    text.push_str("junk\n");
    repo.write(&path, &text);
    repo.commit("docs: kb");
    let w = kb::warnings(repo.state.path(), repo.root(), &repo.cfg, None, &design);
    assert_eq!(
        w,
        [
            format!("kb: 1 invalid lines in {path} (first at line 3); next: cstan kb status"),
            format!("kb: 1 stale and 1 missing entries in {path}; next: cstan kb refresh"),
        ]
    );
    repo.cfg.stale_warn = 3;
    assert_eq!(
        kb::warnings(repo.state.path(), repo.root(), &repo.cfg, None, &design).len(),
        1
    );
    repo.cfg.stale_warn = 1;
    // design record
    repo.cfg.ui_paths = vec!["web/**/*.tsx".to_string()];
    repo.cfg.design_stale_commits = 2;
    let w = kb::warnings(repo.state.path(), repo.root(), &repo.cfg, None, &design);
    assert_eq!(w.len(), 2, "no UI file yet: {w:?}");
    repo.write("web/app/page.tsx", "1\n");
    repo.commit("feat: ui");
    let ui = vec!["web/app/**".to_string()];
    let non_ui = vec!["src/**".to_string()];
    let missing = "design record: DESIGN.md, docs/design/rules.md missing; next: cstan kb build --design and send its brief to a designer agent";
    assert_eq!(
        kb::warnings(repo.state.path(), repo.root(), &repo.cfg, None, &design)[2],
        missing
    );
    assert_eq!(
        kb::warnings(
            repo.state.path(),
            repo.root(),
            &repo.cfg,
            Some(&ui),
            &design
        )[2],
        missing
    );
    assert_eq!(
        kb::warnings(
            repo.state.path(),
            repo.root(),
            &repo.cfg,
            Some(&non_ui),
            &design
        )
        .len(),
        2
    );
    repo.write("DESIGN.md", "d\n");
    repo.write("docs/design/rules.md", "r\n");
    repo.commit("docs: design");
    assert_eq!(
        kb::warnings(repo.state.path(), repo.root(), &repo.cfg, None, &design).len(),
        2
    );
    repo.write("web/app/page.tsx", "2\n");
    repo.commit("feat: ui 2");
    assert_eq!(
        kb::warnings(repo.state.path(), repo.root(), &repo.cfg, None, &design).len(),
        2
    );
    repo.write("web/app/page.tsx", "3\n");
    repo.commit("feat: ui 3");
    assert_eq!(
        kb::warnings(repo.state.path(), repo.root(), &repo.cfg, None, &design)[2],
        "design record: DESIGN.md is 2 UI commits behind; next: cstan kb refresh --design"
    );
    assert_eq!(
        kb::warnings(
            repo.state.path(),
            repo.root(),
            &repo.cfg,
            Some(&non_ui),
            &design
        )
        .len(),
        2
    );
}

#[test]
fn briefs() {
    let repo = Repo::new();
    let v = repo.head();
    let mut entries = vec![entry("fresh", "Fresh", None, &v)];
    for i in 0..80 {
        entries.push(entry(
            &format!("gone_{i:02}"),
            "Gone",
            Some(&format!("src/gone_{i}.rs")),
            &v,
        ));
    }
    repo.put(&entries);
    repo.commit("docs: kb");
    let index = repo.ensure();
    let build = kb::brief(&index, &repo.cfg, BriefKind::Build, false);
    for part in [
        "docs/capstan/kb.jsonl",
        "docs-keeper",
        "cstan kb add",
        "2000",
        "1200",
    ] {
        assert!(build.contains(part), "{part} in {build}");
    }
    let refresh = kb::brief(&index, &repo.cfg, BriefKind::Refresh, false);
    for part in [
        "gone_00",
        "src/gone_0.rs",
        "cstan kb update",
        "--verified",
        "cstan kb remove",
    ] {
        assert!(refresh.contains(part), "{part}");
    }
    assert!(!refresh.contains("- fresh"));
    let refresh_ids = refresh.matches("- gone_").count();
    assert_eq!(refresh_ids, 50);
    let design = kb::brief(&index, &repo.cfg, BriefKind::Build, true);
    assert!(design.contains("designer") && design.contains("cstan design guide"));
    let design_refresh = kb::brief(&index, &repo.cfg, BriefKind::Refresh, true);
    assert!(design_refresh.contains("designer"));
    for text in [&build, &refresh, &design, &design_refresh] {
        assert!(text.len() < 3000, "{}", text.len());
    }
}

#[test]
fn design_warning_above_default_log_cap() {
    use std::io::Write;
    use std::process::Stdio;
    let mut repo = Repo::new();
    repo.write("DESIGN.md", "d\n");
    repo.write("docs/design/rules.md", "r\n");
    repo.write("web/app/page.tsx", "0\n");
    repo.commit("docs: design");
    let mut stream = String::new();
    for i in 0..2010 {
        let body = format!("{}\n", i + 1);
        let from = if i == 0 {
            "from refs/heads/main^0\n"
        } else {
            ""
        };
        stream.push_str(&format!(
            "commit refs/heads/main\ncommitter t <t@t> {} +0000\ndata 2\nui\n{from}M 100644 inline web/app/page.tsx\ndata {}\n{body}\n",
            1_700_000_000 + i,
            body.len()
        ));
    }
    let mut child = Command::new("git")
        .arg("-C")
        .arg(repo.root())
        .args(["fast-import", "--quiet"])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stream.as_bytes())
        .unwrap();
    assert!(child.wait().unwrap().success());
    run(repo.root(), &["reset", "-q", "--hard"]);
    repo.cfg.ui_paths = vec!["web/**/*.tsx".to_string()];
    repo.cfg.design_stale_commits = 2005;
    repo.put(&[]);
    let w = kb::warnings(
        repo.state.path(),
        repo.root(),
        &repo.cfg,
        None,
        &Design::default(),
    );
    assert!(
        w.iter().any(|l| l.contains("is 2005 UI commits behind")),
        "{w:?}"
    );
}

#[test]
fn non_ascii_paths_are_not_quoted() {
    let repo = Repo::new();
    repo.write("src/café.rs", "1\n");
    let v = repo.commit("feat: cafe");
    repo.write("src/café.rs", "2\n");
    repo.commit("fix: cafe");
    repo.write("src/ok \"q\".rs", "1\n");
    let v2 = repo.commit("feat: quoted");
    repo.put(&[
        entry("cafe", "Cafe", Some("src/café.rs"), &v),
        entry("fresh", "Fresh", Some("src/ok \"q\".rs"), &v2),
    ]);
    repo.commit("docs: kb");
    let index = repo.ensure();
    let all = index.for_paths(&["src".to_string()]).unwrap();
    let get = |id: &str| all.iter().find(|h| h.id == id).unwrap().clone();
    assert_eq!(get("cafe").state, "stale");
    assert_eq!(get("fresh").state, "ok");
}

#[test]
fn reuse_lines_keeps_first_hit_and_never_empty_path() {
    let repo = Repo::new();
    repo.write("src/a.rs", "1\n");
    let v = repo.commit("feat: a");
    repo.put(&[entry("a", "Alpha title that is long", Some("src/a.rs"), &v)]);
    repo.commit("docs: kb");
    let index = repo.ensure();
    let owns = vec!["src/**".to_string()];
    let cut = kb::reuse_lines(&index, &owns, 10).unwrap();
    assert_eq!(cut, "- a (helpe");
    // an owned path that is the whole tree has no path to name in the more line
    repo.write("src/b.rs", "1\n");
    repo.commit("feat: b");
    let all = vec!["**".to_string()];
    let text = kb::reuse_lines(&index, &all, 1500).unwrap();
    assert!(
        !text.contains("cstan kb for \n") && !text.ends_with("cstan kb for "),
        "{text}"
    );
}
