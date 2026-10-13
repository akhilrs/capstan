//! The managed `.gitignore` block and the `cstan config check` ignore warnings.
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use cstan_front::gitignore::{self, upsert, Outcome, BLOCK_BEGIN, BLOCK_END};

const ALL: &[&str] = &[".capstan/", "capstan.toml", "capstan.toml~"];
const TOML: &str = "schema_version = 1\n[nexora]\ntrack = \"never\"\n[hosts.claude]\nkind = \"claude\"\n[roles.pm]\nkind = \"PM\"\nhost = \"claude\"\n";

fn block(entries: &[&str]) -> String {
    let body: String = entries.iter().map(|e| format!("{e}\n")).collect();
    format!("{BLOCK_BEGIN}\n{body}{BLOCK_END}\n")
}

#[test]
fn empty_file_gets_only_the_block() {
    assert_eq!(upsert("", ALL).unwrap(), block(ALL));
}

#[test]
fn missing_trailing_newline_is_added_with_a_blank_separator() {
    assert_eq!(
        upsert("target", ALL).unwrap(),
        format!("target\n\n{}", block(ALL))
    );
}

#[test]
fn outside_lines_are_byte_identical_and_stale_entries_replaced() {
    let before = "a\r\nb\n\n";
    let after = "\nc  \r\nlast";
    let old = format!("{before}{}{after}", block(&["old", "stale"]));
    let new = upsert(&old, ALL).unwrap();
    assert_eq!(new, format!("{before}{}{after}", block(ALL)));
}

#[test]
fn second_run_changes_nothing() {
    for existing in ["", "x", "x\n", "x\r\ny\r\n"] {
        let once = upsert(existing, ALL).unwrap();
        assert_eq!(upsert(&once, ALL).unwrap(), once);
    }
}

#[test]
fn track_config_lists_only_the_state_directory() {
    assert_eq!(gitignore::entries(true), vec![".capstan/"]);
    assert_eq!(gitignore::entries(false), ALL.to_vec());
    let old = upsert("", ALL).unwrap();
    assert_eq!(
        upsert(&old, &gitignore::entries(true)).unwrap(),
        block(&[".capstan/"])
    );
}

#[test]
fn crlf_block_keeps_its_line_endings() {
    let old = format!("a\r\n{BLOCK_BEGIN}\r\nold\r\n{BLOCK_END}\r\nb\r\n");
    let new = upsert(&old, ALL).unwrap();
    assert_eq!(
        new,
        format!("a\r\n{BLOCK_BEGIN}\r\n.capstan/\r\ncapstan.toml\r\ncapstan.toml~\r\n{BLOCK_END}\r\nb\r\n")
    );
}

#[test]
fn two_blocks_or_an_unterminated_block_are_refused() {
    let two = format!("{}{}", block(ALL), block(ALL));
    assert!(upsert(&two, ALL).is_err());
    assert!(upsert(&format!("{BLOCK_BEGIN}\n.capstan/\n"), ALL).is_err());
}

fn scratch(label: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("capstan-gitignore-{label}-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn write_reports_what_it_did() {
    let dir = scratch("write");
    let path = dir.join(".gitignore");
    assert_eq!(gitignore::write(&dir, false).unwrap(), Outcome::Created);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), block(ALL));
    assert_eq!(gitignore::write(&dir, false).unwrap(), Outcome::Unchanged);
    assert_eq!(gitignore::write(&dir, true).unwrap(), Outcome::Updated);
    std::fs::write(&path, "node_modules\n").unwrap();
    assert_eq!(gitignore::write(&dir, false).unwrap(), Outcome::Added);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        format!("node_modules\n\n{}", block(ALL))
    );
    std::fs::write(&path, format!("{BLOCK_BEGIN}\n")).unwrap();
    assert!(gitignore::write(&dir, false).is_err());
    std::fs::remove_dir_all(&dir).ok();
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdout(Stdio::null())
        .status()
        .expect("git runs");
    assert!(status.success(), "git {args:?}");
}

fn warnings(dir: &Path) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_cstan"))
        .args(["config", "check"])
        .current_dir(dir)
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CEILING_DIRECTORIES", dir.parent().unwrap())
        .stdin(Stdio::null())
        .output()
        .expect("cstan runs");
    assert_eq!(output.status.code(), Some(0));
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn config_check_warns_until_the_block_is_written() {
    let dir = scratch("check");
    git(&dir, &["init", "-q"]);
    std::fs::write(dir.join("capstan.toml"), TOML).unwrap();
    let before = warnings(&dir);
    assert!(
        before.contains("warning: .capstan/ is not ignored by git"),
        "{before}"
    );
    assert!(
        before.contains("warning: capstan.toml is not ignored by git"),
        "{before}"
    );

    gitignore::write(&dir, false).unwrap();
    let after = warnings(&dir);
    assert!(!after.contains("not ignored by git"), "{after}");

    // Committed capstan.toml with a track_config block: tracked by choice.
    gitignore::write(&dir, true).unwrap();
    git(&dir, &["add", "capstan.toml", ".gitignore"]);
    git(&dir, &["commit", "-q", "-m", "chore: add config"]);
    let tracked = warnings(&dir);
    assert!(
        !tracked.contains("capstan.toml is not ignored"),
        "{tracked}"
    );
    assert!(!tracked.contains(".capstan/ is not ignored"), "{tracked}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn config_check_is_silent_outside_a_git_repository() {
    let dir = scratch("nogit");
    std::fs::write(dir.join("capstan.toml"), TOML).unwrap();
    assert!(!warnings(&dir).contains("not ignored by git"));
    std::fs::remove_dir_all(&dir).ok();
}
