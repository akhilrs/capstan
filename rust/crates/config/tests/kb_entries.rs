//! The `[kb]` table and the knowledge base entry format.
use capstan_config::kb::{
    alias_words, parse_file, remove, split_identifier, to_line, upsert_sorted, validate, Entry,
    Kind,
};
use capstan_config::{parse_config, ConfigError, RoleConfig};
use std::path::Path;

const BASE: &str = r#"schema_version = 1
[nexora]
track = "never"
[hosts.claude]
kind = "claude"
[roles.pm]
kind = "PM"
host = "claude"
[roles.dev]
kind = "Developer"
host = "claude"
[roles.docs-keeper]
kind = "Developer"
host = "claude"
"#;
const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

fn load(extra: &str) -> Result<RoleConfig, String> {
    parse_config(
        format!("{BASE}{extra}").as_bytes(),
        Path::new("/nonexistent"),
    )
    .map_err(|e| match e {
        ConfigError::Invalid(message) => message,
        other => format!("{other:?}"),
    })
}

fn kb(extra: &str) -> capstan_config::KbConfig {
    load(&format!("[kb]\n{extra}"))
        .expect("loads")
        .kb
        .expect("kb")
}

fn refusal(extra: &str) -> String {
    load(&format!("[kb]\n{extra}")).expect_err("refused")
}

fn entry(id: &str) -> Entry {
    Entry {
        id: id.into(),
        kind: Kind::Helper,
        title: "A title".into(),
        body: "A body".into(),
        path: None,
        area: None,
        tags: vec![],
        aliases: vec![],
        related: vec![],
        verified_commit: COMMIT.into(),
    }
}

#[test]
fn no_table_means_none_and_unchanged_json() {
    let config = load("").unwrap();
    assert!(config.kb.is_none());
    assert!(!config.to_json().contains("\"kb\""));
    let with = load("[kb]\n").unwrap();
    assert!(with.to_json().contains("\"kb\""));
}

#[test]
fn defaults_and_bounds() {
    let d = kb("");
    assert!(d.enabled);
    assert_eq!(d.path, "docs/capstan/kb.jsonl");
    assert_eq!(
        (
            d.max_entry_bytes,
            d.max_entries,
            d.search_limit,
            d.reuse_max_bytes,
            d.stale_warn,
            d.design_stale_commits
        ),
        (1200, 2000, 5, 1500, 1, 20)
    );
    assert!(d.ui_paths.is_empty());
    assert_eq!(
        (d.role.as_str(), d.design_role.as_str()),
        ("docs-keeper", "designer")
    );
    for (key, lo, hi) in [
        ("max_entry_bytes", 256, 4000),
        ("max_entries", 1, 10_000),
        ("search_limit", 1, 20),
        ("reuse_max_bytes", 0, 4000),
        ("stale_warn", 1, 10_000),
        ("design_stale_commits", 1, 10_000),
        ("pm_note_minutes", 0, 1440),
        ("log_max_entries", 10, 10_000),
    ] {
        load(&format!("[kb]\n{key} = {lo}\n")).unwrap();
        load(&format!("[kb]\n{key} = {hi}\n")).unwrap();
        assert_eq!(
            refusal(&format!("{key} = {}", hi + 1)),
            format!("kb.{key} must be between {lo} and {hi}")
        );
        assert_eq!(
            refusal(&format!("{key} = {}", lo - 1)),
            format!("kb.{key} must be between {lo} and {hi}")
        );
    }
}

#[test]
fn storage_key() {
    use capstan_config::kb::KbStorage;
    assert_eq!(kb("").storage, KbStorage::Repo);
    assert_eq!(kb("storage = \"local\"").storage, KbStorage::Local);
    assert_eq!(kb("storage = \"repo\"").storage, KbStorage::Repo);
    assert_eq!(
        refusal("storage = \"cloud\""),
        "kb.storage must be \"repo\" or \"local\""
    );
    assert_eq!((kb("").pm_note_minutes, kb("").log_max_entries), (30, 1000));
}

#[test]
fn path_ui_paths_and_roles() {
    assert_eq!(kb("path = \"a/b.jsonl\"").path, "a/b.jsonl");
    for bad in ["/abs/x", "a/../x", "a/", "", "a//b"] {
        assert!(
            refusal(&format!("path = \"{bad}\"")).starts_with("kb.path"),
            "{bad}"
        );
    }
    assert!(refusal(&format!("path = \"{}\"", "a".repeat(201))).starts_with("kb.path"));
    assert_eq!(kb("ui_paths = [\"src/ui/**\"]").ui_paths, vec!["src/ui/**"]);
    assert!(refusal("ui_paths = [\"/x\"]").starts_with("kb.ui_paths[0]"));
    assert!(refusal("ui_paths = [\"a\", \"a\"]").contains("repeats"));
    let many: Vec<String> = (0..33).map(|i| format!("\"p{i}\"")).collect();
    assert!(refusal(&format!("ui_paths = [{}]", many.join(","))).contains("at most 32"));
    assert_eq!(kb("role = \"dev\"").role, "dev");
    assert_eq!(
        refusal("role = \"nobody\""),
        "kb.role \"nobody\" does not name a configured Developer role"
    );
    assert_eq!(
        refusal("role = \"pm\""),
        "kb.role \"pm\" does not name a configured Developer role"
    );
    // design_role is checked only when [design] is configured.
    load("[kb]\ndesign_role = \"nobody\"\n").unwrap();
    assert_eq!(
        load("[design]\n[kb]\ndesign_role = \"nobody\"\n").unwrap_err(),
        "kb.design_role \"nobody\" does not name a configured Developer role"
    );
    assert!(refusal("zz = 1").contains("kb"));
}

#[test]
fn a_valid_file_parses_and_round_trips() {
    let mut e = entry("work-package");
    e.path = Some("rust/src/wp.rs".into());
    e.area = Some("daemon".into());
    e.tags = vec!["a".into()];
    e.aliases = vec!["wp".into()];
    e.related = vec!["other".into(), "path:docs/x.md".into()];
    let text = format!("\n{}\n\n{}\n", to_line(&e), to_line(&entry("zed")));
    let parsed = parse_file(&text, 1200);
    assert!(parsed.invalid.is_empty(), "{:?}", parsed.invalid);
    assert_eq!(parsed.entries[0], e);
    assert_eq!(to_line(&parsed.entries[0]), to_line(&e));
    let bare = to_line(&entry("x"));
    assert_eq!(
        bare,
        format!("{{\"id\":\"x\",\"kind\":\"helper\",\"title\":\"A title\",\"body\":\"A body\",\"verified_commit\":\"{COMMIT}\"}}")
    );
}

#[test]
fn entry_path_allows_a_trailing_slash_but_not_an_empty_segment() {
    let mut e = entry("ok");
    e.path = Some("a/b/".into());
    validate(&e, 256).unwrap();
    e.path = Some("a//b".into());
    assert!(validate(&e, 256).is_err());
    e.path = Some("/".into());
    assert!(validate(&e, 256).is_err());
}

#[test]
fn every_rule_has_its_text() {
    type Change = Box<dyn Fn(&mut Entry)>;
    let cases: Vec<(Change, &str)> = vec![
        (Box::new(|e| e.id = "Bad".into()), "kb entry: id must match ^[a-z0-9][a-z0-9._-]{0,63}$"),
        (Box::new(|e| e.id = "a".repeat(65)), "kb entry: id must match ^[a-z0-9][a-z0-9._-]{0,63}$"),
        (Box::new(|e| e.title = String::new()), "kb entry: title must be 1 to 120 bytes"),
        (Box::new(|e| e.title = "t".repeat(121)), "kb entry: title must be 1 to 120 bytes"),
        (Box::new(|e| e.title = "a\nb".into()), "kb entry: title must be one line"),
        (Box::new(|e| e.body = String::new()), "kb entry: body must not be empty"),
        (Box::new(|e| e.body = "a\nb".into()), "kb entry: body must be one line"),
        (Box::new(|e| e.path = Some("/x".into())), "kb entry: path must be a repo-relative file or directory of at most 200 bytes, without a leading / or a .. segment"),
        (Box::new(|e| e.path = Some("a/../b".into())), "kb entry: path must be a repo-relative file or directory of at most 200 bytes, without a leading / or a .. segment"),
        (Box::new(|e| e.path = Some("a//b".into())), "kb entry: path must be a repo-relative file or directory of at most 200 bytes, without a leading / or a .. segment"),
        (Box::new(|e| e.area = Some("Bad".into())), "kb entry: area must match ^[a-z0-9-]{1,40}$"),
        (Box::new(|e| e.tags = vec!["a".into(); 9]), "kb entry: tags must have at most 8 entries"),
        (Box::new(|e| e.tags = vec!["A".into()]), "kb entry: tags entry must match ^[a-z0-9-]{1,32}$"),
        (Box::new(|e| e.aliases = vec!["a".into(); 9]), "kb entry: aliases must have at most 8 entries"),
        (Box::new(|e| e.aliases = vec!["a".repeat(65)]), "kb entry: aliases entry must be 1 to 64 bytes on one line"),
        (Box::new(|e| e.related = vec!["a".into(); 9]), "kb entry: related must have at most 8 entries"),
        (Box::new(|e| e.related = vec!["Bad".into()]), "kb entry: related entry must be an entry id or path:<repo path>"),
        (Box::new(|e| e.related = vec!["path:/x".into()]), "kb entry: related entry must be an entry id or path:<repo path>"),
        (Box::new(|e| e.verified_commit = "ABC".into()), "kb entry: verified_commit must be 40 lowercase hex characters"),
        (Box::new(|e| e.body = "b".repeat(300)), "kb entry: line is 416 bytes, over the limit of 256"),
    ];
    for (change, want) in cases {
        let mut e = entry("ok");
        change(&mut e);
        assert_eq!(validate(&e, 256).unwrap_err(), want);
    }
    validate(&entry("ok"), 256).unwrap();
}

#[test]
fn bad_lines_are_invalid_with_their_numbers() {
    let ok = to_line(&entry("a"));
    let long = to_line(&{
        let mut e = entry("b");
        e.body = "x".repeat(400);
        e
    });
    let text = format!("{ok}\n\n{ok}\n{long}\nnot json\n{{\"title\":\"x\",\"id\":\"c\"}}\n[]\n");
    let parsed = parse_file(&text, 300);
    assert_eq!(parsed.entries.len(), 1);
    let lines: Vec<usize> = parsed.invalid.iter().map(|(n, _)| *n).collect();
    assert_eq!(lines, vec![3, 4, 5, 6, 7]);
    assert_eq!(
        parsed.invalid[0].1,
        "kb entry: id a repeats the entry on line 1"
    );
    assert!(parsed.invalid[1].1.contains("over the limit of 300"));
    assert!(parsed.invalid[3].1.contains("out of order"));
}

#[test]
fn upsert_sorted_and_remove() {
    let a = to_line(&entry("a"));
    let c = to_line(&entry("c"));
    let odd = "  {\"id\":\"e\", \"kind\":\"helper\"}  ";
    let text = format!("{a}\n{c}\n{odd}\n");
    let b = entry("b");
    let out = upsert_sorted(&text, &b);
    assert_eq!(out, format!("{a}\n{}\n{c}\n{odd}\n", to_line(&b)));
    let mut c2 = entry("c");
    c2.title = "New".into();
    let out = upsert_sorted(&out, &c2);
    assert_eq!(
        out,
        format!("{a}\n{}\n{}\n{odd}\n", to_line(&b), to_line(&c2))
    );
    let z = entry("z");
    assert!(upsert_sorted(&out, &z).ends_with(&format!("{odd}\n{}\n", to_line(&z))));
    assert_eq!(upsert_sorted("", &b), format!("{}\n", to_line(&b)));
    assert_eq!(remove(&text, "c").unwrap(), format!("{a}\n{odd}\n"));
    assert_eq!(remove(&text, "missing"), None);
    assert_eq!(remove(&format!("{a}\n"), "a").unwrap(), "");
}

#[test]
fn upsert_replaces_same_id_in_unsorted_file() {
    let (a, b, c) = (
        to_line(&entry("a")),
        to_line(&entry("b")),
        to_line(&entry("c")),
    );
    let text = format!("{c}\n{a}\n{b}\n");
    let mut b2 = entry("b");
    b2.title = "New".into();
    let out = upsert_sorted(&text, &b2);
    assert_eq!(out, format!("{c}\n{a}\n{}\n", to_line(&b2)));
    assert!(parse_file(&out, 4096)
        .invalid
        .iter()
        .all(|(_, m)| !m.contains("repeats")));
}

#[test]
fn identifiers_split_into_words() {
    let v = |s: &str| split_identifier(s);
    assert_eq!(
        v("work_package_message"),
        ["work", "package", "message", "workpackagemessage"]
    );
    assert_eq!(
        v("WorkPackageMessage"),
        ["work", "package", "message", "workpackagemessage"]
    );
    assert_eq!(v("HTTPServer"), ["http", "server", "httpserver"]);
    assert_eq!(v("kb.jsonl"), ["kb", "jsonl", "kbjsonl"]);
    assert_eq!(v("v2Api"), ["v2", "api", "v2api"]);
    assert_eq!(v("kb"), ["kb"]);
    assert!(v("").is_empty());
    let mut e = entry("work-package");
    e.title = "Send WorkPackage".into();
    e.path = Some("src/file_handoff.rs".into());
    e.aliases = vec!["hand over".into()];
    let words = alias_words(&e);
    for w in [
        "work",
        "package",
        "send",
        "file",
        "handoff",
        "filehandoff",
        "hand",
        "over",
        "handover",
    ] {
        assert!(words.contains(&w.to_string()), "{w}: {words:?}");
    }
    assert_eq!(words.iter().filter(|w| *w == "work").count(), 1);
}
