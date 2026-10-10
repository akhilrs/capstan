//! The live harness never links the operator's `~/.claude` or `~/.claude.json` into the scratch HOME: a real-claude suite
//! gets a scratch `CLAUDE_CONFIG_DIR` with a copy of the sign-in and nothing else. These tests need no Herdr and no Claude
//! Code; they run on a made-up "real" home.

#[path = "common/live_env.rs"]
mod live_env;

use live_env::{claude_login_files, scratch_claude_config, symlinks_into};
use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
use std::path::Path;

fn real_home(root: &Path) -> std::path::PathBuf {
    let home = root.join("real-home");
    std::fs::create_dir_all(home.join(".claude/projects/-some-project")).unwrap();
    std::fs::write(
        home.join(".claude/.credentials.json"),
        "{\"claudeAiOauth\":\"SECRET-TOKEN\"}",
    )
    .unwrap();
    std::fs::write(
        home.join(".claude/settings.json"),
        "{\"permissions\":{\"allow\":[\"Bash(*)\"]}}",
    )
    .unwrap();
    std::fs::write(home.join(".claude/history.jsonl"), "private history\n").unwrap();
    std::fs::write(
        home.join(".claude.json"),
        "{\"oauthAccount\":{\"emailAddress\":\"a@example.invalid\"},\"userID\":\"u1\",\"hasCompletedOnboarding\":true,\"projects\":{\"/home/x\":{\"allowedTools\":[]}},\"cachedStatsigGates\":{}}",
    )
    .unwrap();
    home
}

#[test]
fn the_scratch_claude_config_holds_a_private_copy_of_the_sign_in_and_nothing_else() {
    let root = tempfile::tempdir().unwrap();
    let home = real_home(root.path());
    let config = root.path().join("scratch/claude-config");
    scratch_claude_config(&home, &config).unwrap();

    let mut names: Vec<String> = std::fs::read_dir(&config)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [".claude.json", ".credentials.json", "settings.json"]
    );
    assert_eq!(std::fs::metadata(&config).unwrap().mode() & 0o777, 0o700);
    for name in &names {
        let file = config.join(name);
        assert!(!file.is_symlink(), "{name} is a copy");
        assert_eq!(
            std::fs::metadata(&file).unwrap().mode() & 0o777,
            0o600,
            "{name}"
        );
    }
    assert_eq!(
        std::fs::read_to_string(config.join(".credentials.json")).unwrap(),
        std::fs::read_to_string(home.join(".claude/.credentials.json")).unwrap()
    );
    let state: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(config.join(".claude.json")).unwrap())
            .unwrap();
    let mut keys: Vec<&String> = state.as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(
        keys,
        ["hasCompletedOnboarding", "oauthAccount", "userID"],
        "no project state is copied"
    );
    assert_eq!(
        std::fs::read_to_string(config.join("settings.json"))
            .unwrap()
            .trim(),
        "{}",
        "the settings are minimal, not the operator's"
    );
}

#[test]
fn a_home_without_a_sign_in_is_named_so_the_suite_can_skip() {
    let root = tempfile::tempdir().unwrap();
    let empty = root.path().join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let reason = claude_login_files(&empty).unwrap_err();
    assert!(reason.contains(".credentials.json"), "{reason}");
    assert!(scratch_claude_config(&empty, &root.path().join("config")).is_err());
    assert!(
        !root.path().join("config").exists(),
        "nothing is made for a skipped run"
    );
    // A credentials file without the account state is not a sign-in either.
    let half = root.path().join("half");
    std::fs::create_dir_all(half.join(".claude")).unwrap();
    std::fs::write(half.join(".claude/.credentials.json"), "{}").unwrap();
    assert!(claude_login_files(&half)
        .unwrap_err()
        .contains(".claude.json"));
}

#[test]
fn the_scratch_home_holds_no_symlink_into_the_real_home() {
    let root = tempfile::tempdir().unwrap();
    let real = real_home(root.path());
    let scratch = root.path().join("scratch/home");
    std::fs::create_dir_all(scratch.join(".cache")).unwrap();
    std::fs::write(scratch.join(".bashrc"), "PS1='> '\n").unwrap();
    scratch_claude_config(&real, &root.path().join("scratch/claude-config")).unwrap();
    assert!(symlinks_into(&scratch, &real).is_empty());

    // The old harness linked these; the check finds them, absolute or relative, at any depth.
    symlink(real.join(".claude"), scratch.join(".claude")).unwrap();
    symlink(real.join(".claude.json"), scratch.join(".cache/state.json")).unwrap();
    let found = symlinks_into(&scratch, &real);
    assert_eq!(found.len(), 2, "{found:?}");
    // A link to somewhere else is not the real home's.
    let elsewhere = root.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    symlink(&elsewhere, scratch.join("other")).unwrap();
    assert_eq!(symlinks_into(&scratch, &real).len(), 2);
    let _ = std::fs::set_permissions(&scratch, std::fs::Permissions::from_mode(0o755));
}
