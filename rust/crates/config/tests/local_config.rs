//! `capstan.local.toml` laid over `capstan.toml`: the merge rules, the file a refusal names, and `[remote]` in the tracked
//! file.
use capstan_config::{parse_layered_config, parse_layered_config_with, ConfigError, RoleConfig};
mod common;

use common::{load_trusted, temp_store};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

const BASE: &str = r#"schema_version = 1
[nexora]
track = "never"
[hosts.claude]
kind = "claude"
[roles.pm]
kind = "PM"
host = "claude"
"#;

fn layered(tracked_extra: &str, local: Option<&str>) -> Result<RoleConfig, String> {
    let tracked = format!("{BASE}{tracked_extra}");
    parse_layered_config(
        tracked.as_bytes(),
        local.map(str::as_bytes),
        Path::new("/nonexistent"),
        None,
    )
    .map_err(|error| match error {
        ConfigError::Invalid(message) => message,
        other => format!("{other:?}"),
    })
}

#[test]
fn absent_local_file_is_the_tracked_file() {
    let alone = layered("[remote]\nmessages_per_hour = 7\n", None).unwrap();
    assert_eq!(alone.remote.messages_per_hour, 7);
    assert!(!alone.remote.merge);
}

#[test]
fn tables_merge_key_by_key_and_scalars_replace() {
    let config = layered(
        "[remote]\nmessages_per_hour = 7\n",
        Some("[remote]\nmessages_per_hour = 9\nmerge = true\nmerge_branch = \"main\"\n"),
    )
    .unwrap();
    let remote = &config.remote;
    assert_eq!(remote.messages_per_hour, 9, "the local scalar replaces");
    assert_eq!(
        remote.merge_branch.as_deref(),
        Some("main"),
        "a local key is added"
    );
    assert!(remote.merge, "a local key is added");
}

#[test]
fn nested_tables_merge_recursively() {
    let tracked = "[ntfy.event.verify_failed]\npriority = 3\n";
    let local = "[ntfy]\nenabled = true\nserver = \"https://ntfy.example\"\ntopic = \"t1\"\n[ntfy.event.verify_failed]\ntags = [\"a\"]\n";
    let merged = layered(tracked, Some(local)).unwrap();
    let alone = layered(tracked, None).unwrap();
    let json = merged.to_json();
    assert!(
        json.contains("\"priority\": 3") || json.contains("priority"),
        "{json}"
    );
    assert_ne!(json, alone.to_json(), "the local tags were merged in");
}

#[test]
fn arrays_in_the_local_file_replace() {
    let tracked = "[verify]\ncommands = [\"one\", \"two\"]\n";
    let merged = layered(tracked, Some("[verify]\ncommands = [\"three\"]\n")).unwrap();
    let commands = merged.verify.expect("a [verify] section").commands;
    assert_eq!(commands, vec!["three".to_string()]);
}

#[test]
fn an_unknown_key_names_the_file_it_came_from() {
    let from_local = layered("", Some("[remote]\nbogus = 1\n")).unwrap_err();
    assert!(
        from_local.contains("remote has 1 unknown key(s)"),
        "{from_local}"
    );
    assert!(
        from_local.ends_with("(from capstan.local.toml)"),
        "{from_local}"
    );
    let from_tracked = layered(
        "[remote]\nbogus = 1\n",
        Some("[remote]\nmessages_per_hour = 9\n"),
    )
    .unwrap_err();
    assert!(
        from_tracked.ends_with("(from capstan.toml)"),
        "{from_tracked}"
    );
}

#[test]
fn a_bad_value_names_the_file_and_never_repeats_it() {
    let secret = "tk_SENTINEL_SECRET_VALUE";
    let error = layered(
        "",
        Some(&format!("[remote]\nmessages_per_hour = \"{secret}\"\n")),
    )
    .unwrap_err();
    assert!(error.contains("remote.messages_per_hour"), "{error}");
    assert!(error.ends_with("(from capstan.local.toml)"), "{error}");
    assert!(!error.contains(secret), "{error}");
    let error = layered("", Some(&format!("[ntfy]\ntoken = \"{secret}\"\n"))).unwrap_err();
    assert!(!error.contains(secret), "{error}");
}

#[test]
fn a_syntax_error_names_its_own_file() {
    let error = layered("", Some("[remote\n")).unwrap_err();
    assert!(
        error.starts_with("capstan.local.toml is not valid TOML at line"),
        "{error}"
    );
}

#[test]
fn tracked_remote_merge_or_push_is_refused_with_or_without_a_local_file() {
    for key in ["merge", "push"] {
        let tracked = format!("[remote]\n{key} = true\n");
        for local in [Some("[remote]\nmessages_per_hour = 9\n"), Some(""), None] {
            let error = layered(&tracked, local).unwrap_err();
            assert!(error.contains(&format!("remote.{key} = true")), "{error}");
            assert!(error.contains("capstan.local.toml"), "{error}");
        }
    }
    // false in the tracked file is fine, and the local file turns it on.
    let ok = layered(
        "[remote]\nmerge = false\n",
        Some("[remote]\nmerge = true\n"),
    )
    .unwrap();
    assert!(ok.remote.merge);
}

fn write(dir: &Path, name: &str, text: &str) {
    let file = dir.join(name);
    std::fs::write(&file, text).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
}

#[test]
fn the_loader_reads_the_local_file_next_to_the_tracked_one() {
    let dir = tempfile::tempdir().unwrap();
    let (_holder, store) = temp_store();
    write(dir.path(), "capstan.toml", BASE);
    let plain = load_trusted(dir.path(), &store).unwrap();
    assert!(!plain.remote.merge);
    write(dir.path(), "capstan.local.toml", "[remote]\nmerge = true\n");
    assert!(load_trusted(dir.path(), &store).unwrap().remote.merge);
    // The tracked file alone with merge = true is refused by the loader too.
    write(
        dir.path(),
        "capstan.toml",
        &format!("{BASE}[remote]\nmerge = true\n"),
    );
    let error = load_trusted(dir.path(), &store).unwrap_err().to_string();
    assert!(error.contains("capstan.local.toml"), "{error}");
}

#[test]
fn a_local_file_that_is_not_private_is_refused_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let (_holder, store) = temp_store();
    write(dir.path(), "capstan.toml", BASE);
    write(dir.path(), "capstan.local.toml", "");
    std::fs::set_permissions(
        dir.path().join("capstan.local.toml"),
        std::fs::Permissions::from_mode(0o666),
    )
    .unwrap();
    let error = load_trusted(dir.path(), &store).unwrap_err().to_string();
    assert!(
        error.starts_with("capstan.local.toml must not be writable"),
        "{error}"
    );
}

#[test]
fn tracked_push_url_and_merge_branch_are_refused_and_the_local_file_accepts_them() {
    for (key, value) in [
        ("push_url", "\"ssh://h/r\""),
        ("merge_branch", "\"release/1\""),
    ] {
        let tracked = format!("[remote]\n{key} = {value}\n");
        for local in [Some("[remote]\nmessages_per_hour = 9\n"), Some(""), None] {
            let error = layered(&tracked, local).unwrap_err();
            assert!(
                error.contains(&format!("remote.{key} is not honoured")),
                "{error}"
            );
            assert!(error.contains("capstan.toml"), "{error}");
            assert!(error.contains("capstan.local.toml"), "{error}");
        }
    }
    let ok = layered(
        "",
        Some("[remote]\nmerge = true\nmerge_branch = \"release/1\"\npush = true\npush_url = \"ssh://h/r\"\n"),
    )
    .unwrap();
    assert_eq!(ok.remote.push_url.as_deref(), Some("ssh://h/r"));
    assert_eq!(ok.remote.merge_branch.as_deref(), Some("release/1"));
}

#[test]
fn without_git_confirmation_the_local_remote_destinations_and_switches_are_refused() {
    let tracked = BASE.to_string();
    for local in [
        "[remote]\nmerge = true\n",
        "[remote]\npush = true\n",
        "[remote]\nmerge_branch = \"x\"\n",
        "[remote]\nmerge = true\npush = true\npush_url = \"ssh://h/r\"\n",
    ] {
        let error = parse_layered_config_with(
            tracked.as_bytes(),
            Some(local.as_bytes()),
            Path::new("/nonexistent"),
            None,
            false,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("could not confirm"), "{local}: {error}");
        assert!(error.contains("capstan.local.toml"), "{error}");
    }
    // Everything else in the local file, and merge = false, stay honoured.
    let ok = parse_layered_config_with(
        tracked.as_bytes(),
        Some(b"[remote]\nmerge = false\nmessages_per_hour = 9\n"),
        Path::new("/nonexistent"),
        None,
        false,
    )
    .unwrap();
    assert_eq!(ok.remote.messages_per_hour, 9);
}

fn git_repo(dir: &Path) {
    for args in [
        &["init", "-q"][..],
        &["config", "user.email", "t@example.com"],
        &["config", "user.name", "T"],
    ] {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}");
    }
}

fn git_add(dir: &Path, path: &str) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["add", "-f", "--", path])
        .output()
        .unwrap();
    assert!(out.status.success());
}

#[test]
fn a_local_file_that_git_tracks_is_refused_and_an_untracked_one_is_honoured() {
    let dir = tempfile::tempdir().unwrap();
    let (_holder, store) = temp_store();
    git_repo(dir.path());
    write(dir.path(), "capstan.toml", BASE);
    write(
        dir.path(),
        "capstan.local.toml",
        "[remote]\nmerge = true\npush = true\npush_url = \"ssh://h/r\"\n",
    );
    let honoured = load_trusted(dir.path(), &store).unwrap();
    assert!(honoured.remote.merge && honoured.remote.push);
    git_add(dir.path(), "capstan.local.toml");
    let error = load_trusted(dir.path(), &store).unwrap_err().to_string();
    assert!(
        error.contains("capstan.local.toml is tracked in git"),
        "{error}"
    );
    assert!(
        error.contains("git rm --cached capstan.local.toml"),
        "{error}"
    );
}

#[test]
fn a_local_file_outside_any_git_work_tree_is_honoured() {
    let dir = tempfile::tempdir().unwrap();
    let (_holder, store) = temp_store();
    write(dir.path(), "capstan.toml", BASE);
    write(dir.path(), "capstan.local.toml", "[remote]\nmerge = true\n");
    assert!(load_trusted(dir.path(), &store).unwrap().remote.merge);
}

const NTFY_LOCAL_ONLY: [(&str, &str); 8] = [
    ("server", "\"https://attacker.example\""),
    ("topic", "\"t\""),
    ("user", "\"u\""),
    ("token_env", "\"GH_TOKEN\""),
    ("token_file", "\"/home/x/.ssh/id_ed25519\""),
    ("password_env", "\"PW\""),
    ("password_file", "\"/home/x/pw\""),
    ("ca_file", "\"/etc/ca.pem\""),
];

#[test]
fn tracked_ntfy_connection_keys_and_enabled_and_env_pass_are_refused() {
    let mut cases: Vec<(String, String)> = NTFY_LOCAL_ONLY
        .iter()
        .map(|(k, v)| (format!("[ntfy]\n{k} = {v}\n"), format!("ntfy.{k}")))
        .collect();
    cases.push((
        "[ntfy]\nenabled = true\nserver = \"https://a.example\"\ntopic = \"t\"\n".into(),
        "ntfy.".into(),
    ));
    cases.push((
        "[ntfy]\nenabled = true\n".into(),
        "ntfy.enabled = true".into(),
    ));
    cases.push(("[env]\npass = [\"MY_KEY\"]\n".into(), "env.pass".into()));
    cases.push(("[env]\npass = []\n".into(), "env.pass".into()));
    for (tracked, name) in cases {
        for local in [Some("[remote]\nmessages_per_hour = 9\n"), Some(""), None] {
            let error = layered(&tracked, local).unwrap_err();
            assert!(error.contains(&name), "{tracked}: {error}");
            assert!(error.contains("is not honoured in capstan.toml"), "{error}");
            assert!(error.contains("capstan.local.toml"), "{error}");
        }
    }
}

#[test]
fn tracked_ntfy_display_keys_and_disabled_stay_shared() {
    let ok = layered(
        "[ntfy]\nenabled = false\nevents = [\"agent_lost\"]\ntitle_prefix = \"p\"\ntimeout_seconds = 5\nretries = 1\n[ntfy.event.agent_lost]\npriority = \"urgent\"\n",
        None,
    )
    .unwrap();
    assert!(!ok.ntfy.enabled);
}

#[test]
fn the_local_file_accepts_ntfy_connection_keys_and_env_pass() {
    let ok = layered(
        "",
        Some("[ntfy]\nenabled = true\nserver = \"https://ntfy.example.org\"\ntopic = \"t\"\ntoken_env = \"NTFY_TOKEN\"\n[env]\npass = [\"MY_KEY\"]\n"),
    )
    .unwrap();
    assert!(ok.ntfy.enabled);
    assert_eq!(ok.env_pass, vec!["MY_KEY".to_string()]);
}

#[test]
fn without_git_confirmation_local_ntfy_connection_keys_and_env_pass_are_refused() {
    let mut locals: Vec<String> = NTFY_LOCAL_ONLY
        .iter()
        .map(|(k, v)| format!("[ntfy]\n{k} = {v}\n"))
        .collect();
    locals.push("[ntfy]\nenabled = true\n".into());
    locals.push("[env]\npass = [\"MY_KEY\"]\n".into());
    for local in locals {
        let error = parse_layered_config_with(
            BASE.as_bytes(),
            Some(local.as_bytes()),
            Path::new("/nonexistent"),
            None,
            false,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("could not confirm"), "{local}: {error}");
    }
    let ok = parse_layered_config_with(
        BASE.as_bytes(),
        Some(b"[ntfy]\nenabled = false\nretries = 1\n"),
        Path::new("/nonexistent"),
        None,
        false,
    )
    .unwrap();
    assert!(!ok.ntfy.enabled);
}
