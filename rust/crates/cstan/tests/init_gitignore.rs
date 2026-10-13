//! `cstan init` and the managed `.gitignore` block: the block on a fresh init, `--gitignore`, `--track-config`, `--git`,
//! a legacy project that still names `roles/designer.md`, and a refused block.
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use cstan_front::gitignore::{BLOCK_BEGIN, BLOCK_END};

const BLOCK: &str = "# >>> capstan (managed by cstan init)\n.capstan/\ncapstan.toml\ncapstan.toml~\n# <<< capstan\n";
const TRACKED_BLOCK: &str = "# >>> capstan (managed by cstan init)\n.capstan/\n# <<< capstan\n";

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cstan-init-gi-{}-{name}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn cstan(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cstan"))
        .args(args)
        .current_dir(dir)
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("HOME", dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "T")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "T")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .env("GIT_CEILING_DIRECTORIES", dir.parent().unwrap())
        .stdin(Stdio::null())
        .output()
        .expect("cstan runs")
}

fn out(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn err(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git runs");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn designer_source(dir: &Path) -> String {
    let check = cstan(dir, &["config", "check"]);
    let json: serde_json::Value = serde_json::from_slice(&check.stdout).expect("config check json");
    let roles = json["roles"].as_array().expect("roles");
    let designer = roles
        .iter()
        .find(|r| r["name"] == "designer")
        .expect("designer role");
    designer["prompt"]["source"].as_str().unwrap().to_string()
}

#[test]
fn a_fresh_init_writes_the_block_and_no_roles_folder() {
    let dir = scratch("fresh");
    let init = cstan(&dir, &["init"]);
    assert_eq!(init.status.code(), Some(0), "{}", err(&init));
    let text = out(&init);
    assert!(
        text.contains("Created .gitignore with the capstan block\n"),
        "{text}"
    );
    assert!(!text.contains("roles/designer.md"), "{text}");
    assert!(!dir.join("roles").exists());
    assert_eq!(
        std::fs::read_to_string(dir.join(".gitignore")).unwrap(),
        BLOCK
    );
    assert_eq!(designer_source(&dir), "builtin");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_existing_gitignore_keeps_every_earlier_line() {
    for existing in ["target\r\nnode_modules/\n\n  x", "target\nnode_modules/\n"] {
        let dir = scratch("existing");
        std::fs::write(dir.join(".gitignore"), existing).unwrap();
        let init = cstan(&dir, &["init"]);
        assert_eq!(init.status.code(), Some(0), "{}", err(&init));
        assert!(out(&init).contains("Added the capstan block to .gitignore\n"));
        let now = std::fs::read_to_string(dir.join(".gitignore")).unwrap();
        let separator = if existing.ends_with('\n') { "" } else { "\n" };
        assert_eq!(now, format!("{existing}{separator}\n{BLOCK}"));
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[test]
fn gitignore_flag_twice_leaves_one_block_and_reports_current() {
    let dir = scratch("twice");
    assert_eq!(cstan(&dir, &["init"]).status.code(), Some(0));
    let first = cstan(&dir, &["init", "--gitignore"]);
    assert_eq!(first.status.code(), Some(0));
    assert!(out(&first).contains("The capstan block in .gitignore is current\n"));
    let before = std::fs::read(dir.join(".gitignore")).unwrap();
    let second = cstan(&dir, &["init", "--gitignore"]);
    assert_eq!(second.status.code(), Some(0));
    assert_eq!(
        out(&second).lines().next(),
        Some("The capstan block in .gitignore is current")
    );
    assert_eq!(std::fs::read(dir.join(".gitignore")).unwrap(), before);
    let text = String::from_utf8(before).unwrap();
    assert_eq!(text.matches(BLOCK_BEGIN).count(), 1);
    assert_eq!(text.matches(BLOCK_END).count(), 1);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn gitignore_flag_works_before_init_and_changes_the_block_with_track_config() {
    let dir = scratch("flag");
    let created = cstan(&dir, &["init", "--gitignore"]);
    assert_eq!(created.status.code(), Some(0));
    assert_eq!(out(&created), "Created .gitignore with the capstan block\n");
    assert!(!dir.join(".capstan").exists());
    let tracked = cstan(&dir, &["--track-config", "init", "--gitignore"]);
    assert_eq!(tracked.status.code(), Some(2), "flags go after the command");
    let tracked = cstan(&dir, &["init", "--track-config", "--gitignore"]);
    assert_eq!(tracked.status.code(), Some(0));
    assert!(out(&tracked).starts_with("Updated the capstan block in .gitignore\n"));
    assert!(
        out(&tracked).contains("capstan.toml is not ignored; commit it to share the configuration")
    );
    assert_eq!(
        std::fs::read_to_string(dir.join(".gitignore")).unwrap(),
        TRACKED_BLOCK
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn gitignore_with_git_and_unknown_flags_are_usage_errors() {
    let dir = scratch("usage");
    for args in [
        &["init", "--gitignore", "--git"][..],
        &["init", "--track-config", "--bogus"][..],
    ] {
        let output = cstan(&dir, args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
    }
    assert!(!dir.join(".gitignore").exists() && !dir.join(".capstan").exists());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn track_config_keeps_capstan_toml_out_of_the_block_and_git_commits_it() {
    let dir = scratch("track");
    let init = cstan(&dir, &["init", "--git", "--track-config"]);
    assert_eq!(init.status.code(), Some(0), "{}", err(&init));
    assert!(
        out(&init).contains("capstan.toml is not ignored; commit it to share the configuration")
    );
    assert_eq!(
        std::fs::read_to_string(dir.join(".gitignore")).unwrap(),
        TRACKED_BLOCK
    );
    let files = git(&dir, &["ls-tree", "-r", "--name-only", "HEAD"]);
    assert_eq!(files, ".gitignore\ncapstan.toml\n");
    assert_eq!(git(&dir, &["check-ignore", "-q", "capstan.toml"]), "");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn plain_git_commits_the_gitignore_but_not_the_ignored_config() {
    let dir = scratch("git");
    let init = cstan(&dir, &["init", "--git"]);
    assert_eq!(init.status.code(), Some(0), "{}", err(&init));
    assert_eq!(
        git(&dir, &["ls-tree", "-r", "--name-only", "HEAD"]),
        ".gitignore\n"
    );
    assert!(dir.join("capstan.toml").is_file());
    assert_eq!(git(&dir, &["status", "--porcelain"]), "");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_legacy_project_with_a_designer_prompt_file_still_loads_it() {
    let dir = scratch("legacy");
    assert_eq!(cstan(&dir, &["init"]).status.code(), Some(0));
    let starter = std::fs::read_to_string(dir.join("capstan.toml")).unwrap();
    let legacy = starter.replace(
        "[roles.designer]\n",
        "[roles.designer]\nprompt_file = \"roles/designer.md\"\n",
    );
    assert_ne!(legacy, starter);
    std::fs::write(dir.join("capstan.toml"), legacy).unwrap();
    std::fs::create_dir_all(dir.join("roles")).unwrap();
    std::fs::write(dir.join("roles/designer.md"), "My own designer prompt.\n").unwrap();
    assert_eq!(designer_source(&dir), "file");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_existing_designer_file_is_left_alone_and_unused() {
    let dir = scratch("kept");
    std::fs::create_dir_all(dir.join("roles")).unwrap();
    std::fs::write(dir.join("roles/designer.md"), "Mine.\n").unwrap();
    assert_eq!(cstan(&dir, &["init"]).status.code(), Some(0));
    assert_eq!(
        std::fs::read_to_string(dir.join("roles/designer.md")).unwrap(),
        "Mine.\n"
    );
    assert_eq!(designer_source(&dir), "builtin");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_refused_block_fails_with_exit_3_and_writes_nothing_else() {
    for args in [&["init"][..], &["init", "--gitignore"][..]] {
        let dir = scratch("refused");
        let broken = format!("{BLOCK}{BLOCK}");
        std::fs::write(dir.join(".gitignore"), &broken).unwrap();
        let output = cstan(&dir, args);
        assert_eq!(output.status.code(), Some(3), "{args:?}: {}", err(&output));
        let message = err(&output);
        assert!(
            message.contains(".gitignore") && message.contains(BLOCK_BEGIN),
            "{message}"
        );
        assert!(!dir.join(".capstan").exists() && !dir.join("capstan.toml").exists());
        assert_eq!(
            std::fs::read_to_string(dir.join(".gitignore")).unwrap(),
            broken
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[test]
fn config_check_stops_warning_after_init_gitignore() {
    let dir = scratch("fix");
    git(&dir, &["init", "-q"]);
    std::fs::write(dir.join("capstan.toml"), cstan_config_starter()).unwrap();
    let before = err(&cstan(&dir, &["config", "check"]));
    assert!(before.contains("not ignored by git"), "{before}");
    assert_eq!(cstan(&dir, &["init", "--gitignore"]).status.code(), Some(0));
    let after = err(&cstan(&dir, &["config", "check"]));
    assert!(!after.contains("not ignored by git"), "{after}");
    std::fs::remove_dir_all(&dir).ok();
}

fn cstan_config_starter() -> &'static str {
    capstan_config::starter::STARTER_CONFIG
}
