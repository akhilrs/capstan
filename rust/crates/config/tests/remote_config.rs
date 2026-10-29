//! The `[remote]` section: defaults, each key, each rule of the push URL and the branch name, and the documentation
//! tables. Rust-only: no corpus case contains `[remote]`.
use capstan_config::remote::{
    parse_branch_table, validate_branch_name, validate_push_url, DOC_FAILURE_REASONS,
    DOC_PUSH_REASONS, DOC_REFUSAL_CODES, DOC_UNSAFE_FAMILIES,
};
use capstan_config::{accepted_keys, parse_config, ConfigError, Remote, RoleConfig};
use std::path::Path;

const BASE: &str = r#"schema_version = 1
[nexora]
track = "never"
[hosts.claude]
kind = "claude"
[roles.pm]
kind = "PM"
host = "claude"
"#;

fn load(extra: &str) -> Result<RoleConfig, String> {
    let text = format!("{BASE}{extra}");
    parse_config(text.as_bytes(), Path::new("/nonexistent")).map_err(|error| match error {
        ConfigError::Invalid(message) => message,
        other => format!("{other:?}"),
    })
}

fn url_section(url: &str, push: bool) -> String {
    format!(
        "[remote]\nmerge = true\npush = {push}\npush_url = {}\n",
        toml_string(url)
    )
}

#[test]
fn an_absent_section_is_the_default_and_leaves_the_json_unchanged() {
    let config = load("").expect("loads");
    assert_eq!(config.remote, Remote::default());
    assert!(!config.remote.merge && !config.remote.push);
    assert_eq!(config.remote.merge_branch, None);
    assert_eq!(config.remote.push_url, None);
    assert_eq!(config.remote.messages_per_hour, 30);
    assert!(!config.to_json().contains("remote"));
    let explicit = load("[remote]\nmerge = false\npush = false\nmessages_per_hour = 30\n").unwrap();
    assert_eq!(explicit.to_json(), config.to_json());
}

#[test]
fn each_key_is_read_and_a_non_default_section_is_printed() {
    let config = load(
        "[remote]\nmerge = true\nmerge_branch = \"release/1\"\npush = true\npush_url = \"ssh://h/r\"\nmessages_per_hour = 600\n",
    )
    .expect("loads");
    assert_eq!(
        config.remote,
        Remote {
            merge: true,
            merge_branch: Some("release/1".into()),
            push: true,
            push_url: Some("ssh://h/r".into()),
            messages_per_hour: 600,
        }
    );
    let json = config.to_json();
    assert!(json.contains("\"remote\""), "{json}");
    assert!(json.contains("\"messagesPerHour\": 600"), "{json}");
}

#[test]
fn the_role_hash_ignores_the_section() {
    let without = load("").unwrap();
    let with = load("[remote]\nmerge = true\nmerge_branch = \"x\"\n").unwrap();
    assert!(!without.roles.is_empty());
    for (a, b) in without.roles.iter().zip(&with.roles) {
        assert_eq!(a.config_hash, b.config_hash);
    }
}

#[test]
fn unknown_keys_and_wrong_types_name_their_key() {
    assert!(load("[remote]\nzz = 1\n")
        .unwrap_err()
        .contains("remote has 1 unknown key"));
    for (extra, key) in [
        ("[remote]\nmerge = 1\n", "remote.merge"),
        ("[remote]\npush = \"yes\"\n", "remote.push"),
        ("[remote]\nmerge_branch = 3\n", "remote.merge_branch"),
        ("[remote]\npush_url = 3\n", "remote.push_url"),
        (
            "[remote]\nmessages_per_hour = 0\n",
            "remote.messages_per_hour",
        ),
        (
            "[remote]\nmessages_per_hour = 601\n",
            "remote.messages_per_hour",
        ),
        (
            "[remote]\nmessages_per_hour = \"3\"\n",
            "remote.messages_per_hour",
        ),
        ("[remote]\nmerge_branch = \"a..b\"\n", "remote.merge_branch"),
        ("[remote]\nmerge_branch = \"\"\n", "remote.merge_branch"),
        ("[remote]\npush_url = \"http://h/r\"\n", "remote.push_url"),
    ] {
        let error = load(extra).expect_err(extra);
        assert!(error.contains(key), "{extra}: {error}");
    }
}

#[test]
fn a_remote_that_is_not_a_table_is_refused() {
    let text = format!("remote = 1\n{BASE}");
    let error = parse_config(text.as_bytes(), Path::new("/nonexistent")).unwrap_err();
    assert!(
        format!("{error:?}").contains("remote must be a table"),
        "{error:?}"
    );
}

#[test]
fn push_needs_merge_and_a_url() {
    let no_merge = load("[remote]\npush = true\npush_url = \"ssh://h/r\"\n").unwrap_err();
    assert!(
        no_merge.contains("remote.push") && no_merge.contains("remote.merge"),
        "{no_merge}"
    );
    let no_url = load("[remote]\nmerge = true\npush = true\n").unwrap_err();
    assert!(no_url.contains("remote.push_url"), "{no_url}");
    assert!(load(&url_section("ssh://h/r", true)).is_ok());
}

#[test]
fn a_branch_is_validated_when_set_and_ignored_while_merge_is_off() {
    assert!(load("[remote]\nmerge_branch = \"feature/x\"\n").is_ok());
    assert!(load("[remote]\nmerge = false\nmerge_branch = \"a..b\"\n").is_err());
    let config = load("[remote]\nmerge = false\nmerge_branch = \"main\"\n").unwrap();
    assert!(!config.remote.merge);
    assert_eq!(config.remote.merge_branch.as_deref(), Some("main"));
}

#[test]
fn push_urls_are_accepted_and_refused_by_the_rules() {
    let long = format!("ssh://h/{}", "a".repeat(1025));
    let accepted = [
        "file:///abs/path",
        "ssh://host:2222/r",
        "ssh://[::1]/r",
        "ssh://[::1]:22/r",
        "FILE:///x",
        "HTTPS://h/r",
        "host:a::b",
        "cd:path",
        "https://x/",
        "file:///x",
        "git@github.com:org/repo.git",
        "ssh://git@host/r",
        "file://localhost/x",
        "ssh://h:65535/r",
    ];
    let refused = [
        "https:///x",
        "http://h/r",
        "git://h/r",
        "user:pw@host:path",
        "ssh://user:pw@h/r",
        "ssh://h/r ",
        long.as_str(),
        "-oProxyCommand=x:y",
        "user@-oProxyCommand=x:y",
        "ext::x",
        "EXT::x",
        "hg::x",
        "a::b",
        "https:/x",
        "file:/x",
        "host:dir@x",
        "ssh://h:99999/r",
        "ssh://h:0/r",
        "ssh://h:abc/r",
        "ssh://[zz]/r",
        "ssh://h/\u{7f}x",
        "c:path",
        "ssh:x",
        "ssh://-oProxyCommand/r",
        "ssh://h/r?x",
        "ssh://h/r#x",
        "ssh://h/r%20x",
        "ssh:///r",
        "",
        "plain/path",
        "host:",
        "ftp://h/r",
        "file://",
        "file://h:22/x",
        "ssh://[::1/r",
        "ssh://@h/r",
        "ssh://a@b@h/r",
        "[::1]:path",
    ];
    for url in accepted {
        assert!(validate_push_url(url).is_ok(), "{url}");
        assert!(load(&url_section(url, false)).is_ok(), "{url}");
        assert!(load(&url_section(url, true)).is_ok(), "{url}");
    }
    for url in refused {
        assert!(validate_push_url(url).is_err(), "{url:?}");
        let error = load(&url_section(url, false)).expect_err(url);
        assert!(error.contains("remote.push_url"), "{url:?}: {error}");
    }
}

#[test]
fn a_refused_password_is_never_echoed() {
    for url in ["ssh://user:pw@h/r", "user:pw@host:path"] {
        let error = load(&url_section(url, false)).unwrap_err();
        assert!(!error.contains("pw"), "{error}");
        assert!(!error.contains(url), "{error}");
    }
}

#[test]
fn the_branch_table_round_trips_and_binds_the_config() {
    let parsed =
        parse_branch_table("accept\ta\\\\b\naccept\ta\\tb\ngit-refuses\ta\\x01b\n\n# note\n");
    assert_eq!(
        parsed,
        vec![
            ("accept".to_string(), "a\\b".to_string()),
            ("accept".to_string(), "a\tb".to_string()),
            ("git-refuses".to_string(), "a\u{1}b".to_string()),
        ]
    );
    let table = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/branch_names.txt"),
    )
    .unwrap();
    let rows = parse_branch_table(&table);
    assert!(rows.iter().any(|(class, _)| class == "config-refuses"));
    assert!(rows.iter().any(|(_, name)| name.contains('\u{7f}')));
    for (class, name) in rows {
        let verdict = validate_branch_name(&name);
        if class == "accept" {
            assert!(verdict.is_ok(), "{name:?}");
        } else {
            assert!(verdict.is_err(), "{class} {name:?}");
        }
        let section = format!("[remote]\nmerge_branch = {}\n", toml_string(&name));
        assert_eq!(load(&section).is_ok(), class == "accept", "{name:?}");
    }
}

fn toml_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\u{:04x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[test]
fn accepted_keys_lists_the_section() {
    let keys = accepted_keys();
    for key in [
        "remote.merge",
        "remote.merge_branch",
        "remote.push",
        "remote.push_url",
        "remote.messages_per_hour",
    ] {
        assert!(keys.iter().any(|k| k.path == key), "{key}");
    }
}

/// The values of the one-column table under `heading`: the rows of the first table after it, up to the first blank line.
fn doc_table(doc: &str, heading: &str) -> Vec<String> {
    let mut lines = doc.lines().skip_while(|line| *line != heading).skip(1);
    let mut rows = Vec::new();
    let mut started = false;
    for line in lines.by_ref() {
        if line.starts_with('|') {
            started = true;
            rows.push(line.to_string());
        } else if started {
            break;
        } else if line.starts_with('#') {
            panic!("no table under {heading}");
        }
    }
    assert!(rows.len() >= 2, "{heading}");
    let cells: Vec<String> = rows
        .iter()
        .map(|row| row.trim().trim_matches('|').trim().to_string())
        .collect();
    assert_eq!(cells[0], "Value", "{heading}");
    assert!(cells[1].starts_with("---"), "{heading}");
    cells[2..]
        .iter()
        .map(|cell| {
            cell.strip_prefix('`')
                .and_then(|c| c.strip_suffix('`'))
                .unwrap_or_else(|| panic!("{heading}: cell {cell:?} is not backticked"))
                .to_string()
        })
        .collect()
}

#[test]
fn the_documentation_tables_hold_exactly_the_constants() {
    let doc = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../docs/reference/configuration.md"),
    )
    .unwrap();
    let tables: [(&str, &[&str]); 4] = [
        ("### Refused local config keys", DOC_UNSAFE_FAMILIES),
        ("### Refusal codes", DOC_REFUSAL_CODES),
        ("### Failure reasons", DOC_FAILURE_REASONS),
        ("### Push reasons", DOC_PUSH_REASONS),
    ];
    for (heading, constant) in tables {
        let mut unique = constant.to_vec();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            unique.len(),
            constant.len(),
            "{heading}: duplicate in the const"
        );
        assert_eq!(doc_table(&doc, heading), constant, "{heading}");
    }
    let plain = DOC_UNSAFE_FAMILIES
        .iter()
        .filter(|f| f.starts_with("http.") && !f.starts_with("http.*."))
        .count();
    let starred = DOC_UNSAFE_FAMILIES
        .iter()
        .filter(|f| f.starts_with("http.*."))
        .count();
    assert_eq!((plain, starred), (13, 13));
    for sentence in [
        "Root git calls trust the user's global and system git config, which any program running as the same user can change.",
        "Review `[remote]` before running `cstan` in a repository you did not write",
        "(`push_url_rewritten`)",
        "`DBUS_SESSION_BUS_ADDRESS` or `XDG_RUNTIME_DIR`",
        "often committed",
    ] {
        assert!(doc.contains(sentence), "{sentence}");
    }
}

fn configuration_doc() -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../docs/reference/configuration.md"),
    )
    .unwrap()
}

#[test]
fn the_failure_reasons_and_unsafe_families_are_fixed() {
    assert_eq!(
        DOC_FAILURE_REASONS,
        ["merge_failed", "index_lock_left", "timeout"]
    );
    for key in [
        "remote.*.pushurl",
        "extensions.partialClone",
        "remote.*.promisor",
        "remote.*.partialclonefilter",
        "core.sshCommand",
        "core.gitProxy",
        "core.askPass",
        "remote.*.receivepack",
        "remote.*.uploadpack",
        "remote.*.vcs",
        "remote.*.proxy",
        "credential.*",
        "url.*.insteadOf",
        "url.*.pushInsteadOf",
        "include.path",
        "includeIf.*",
    ] {
        assert!(DOC_UNSAFE_FAMILIES.contains(&key), "{key}");
    }
    for entry in DOC_UNSAFE_FAMILIES {
        let parts: Vec<&str> = entry.split('.').collect();
        let ok = match parts.as_slice() {
            [s, v] => !s.contains('*') && (!v.contains('*') || *v == "*"),
            [s, mid, v] => !s.contains('*') && *mid == "*" && !v.contains('*'),
            _ => false,
        };
        assert!(ok && parts.iter().all(|p| !p.is_empty()), "{entry}");
    }
}

#[test]
fn the_safety_section_lists_every_push_family() {
    let doc = configuration_doc();
    let mut lines = doc
        .lines()
        .skip_while(|l| *l != "## Merge and push safety")
        .skip(1);
    let mut rows = Vec::new();
    for line in lines.by_ref() {
        if line.starts_with("## ") {
            panic!("no table in the safety section");
        }
        if line.starts_with('|') {
            rows.push(line.to_string());
            break;
        }
    }
    rows.extend(
        lines
            .by_ref()
            .take_while(|l| l.starts_with('|'))
            .map(String::from),
    );
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            r.trim()
                .trim_matches('|')
                .split('|')
                .map(|c| c.trim().to_string())
                .collect()
        })
        .collect();
    assert_eq!(cells[0], ["Key", "Scope"]);
    let mut push_keys = Vec::new();
    for row in &cells[2..] {
        assert_eq!(row.len(), 2, "{row:?}");
        assert!(
            ["merge", "push", "both"].contains(&row[1].as_str()),
            "{row:?}"
        );
        if row[1] != "merge" {
            let key = row[0].trim_matches('`').to_string();
            push_keys.push(key);
        }
    }
    let mut want: Vec<String> = DOC_UNSAFE_FAMILIES.iter().map(|s| s.to_string()).collect();
    want.sort();
    push_keys.sort();
    assert_eq!(push_keys, want);
    let section: String = doc
        .lines()
        .skip_while(|l| *l != "## Merge and push safety")
        .skip(1)
        .take_while(|l| !l.starts_with("## "))
        .collect::<Vec<_>>()
        .join("\n");
    for heading in [
        "### Allowed in merges",
        "### Refused setups",
        "### Residuals",
        "### Re-check",
    ] {
        assert!(section.lines().any(|l| l == heading), "{heading}");
    }
    let mut tags = Vec::new();
    for heading in ["### Refused setups", "### Residuals"] {
        for line in section
            .lines()
            .skip_while(|l| *l != heading)
            .skip(1)
            .take_while(|l| !l.starts_with('#'))
            .filter(|l| l.starts_with("- "))
        {
            let tag = line[2..]
                .strip_prefix('`')
                .and_then(|r| r.split_once('`'))
                .unwrap_or_else(|| panic!("untagged bullet {line}"))
                .0;
            tags.push(tag.to_string());
        }
    }
    assert_eq!(
        tags,
        [
            "alternates",
            "git-lfs",
            "sha256",
            "shallow-grafts",
            "safe-directory",
            "global-config",
            "dbus",
            "home",
            "batchmode",
            "group-kill",
            "sparse-checkout",
            "isolated-cost",
            "identity",
            "gitfile-outside",
            "http-alternates",
            "lc-all",
            "credential-helper",
        ]
    );
}

fn visit(dir: &Path, words: &[String], hits: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            visit(&path, words, hits);
        } else if let Ok(text) = std::fs::read_to_string(&path) {
            for word in words {
                if text.contains(word.as_str()) {
                    hits.push(format!("{}: {word}", path.display()));
                }
            }
        }
    }
}

#[test]
fn the_removed_failure_reasons_appear_nowhere() {
    let words = [
        ["tree", "mismatch"].join("_"),
        ["no", "merge", "head"].join("_"),
    ];
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let mut hits = Vec::new();
    visit(&root.join("docs"), &words, &mut hits);
    if let Ok(crates) = std::fs::read_dir(root.join("rust/crates")) {
        for krate in crates.flatten() {
            for sub in ["src", "tests"] {
                visit(&krate.path().join(sub), &words, &mut hits);
            }
        }
    }
    assert!(hits.is_empty(), "{hits:?}");
}
