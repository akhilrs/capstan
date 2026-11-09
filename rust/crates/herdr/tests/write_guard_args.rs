//! The Claude arguments of a PM with the memory write guard.

use capstan_herdr::claude_args::{claude_arguments, guard_path_is_usable, ClaudeRoleSettings};
use serde_json::Value;

fn pm(hooks: &str, guard: Option<&str>) -> ClaudeRoleSettings {
    ClaudeRoleSettings {
        model: None,
        permission_mode: "default".into(),
        allow: vec!["Bash(cstan *)".into()],
        deny: vec!["NotebookEdit".into(), "Agent".into(), "Task".into()],
        hooks: hooks.into(),
        mcp: vec![],
        write_guard: guard.map(str::to_string),
        write_dirs: Vec::new(),
    }
}

fn settings(args: &[String]) -> Value {
    let at = args.iter().position(|a| a == "--settings").unwrap();
    serde_json::from_str(&args[at + 1]).unwrap()
}

#[test]
fn guard_adds_the_memory_allow_rule_and_only_the_guard_hook() {
    let dir = "/home/u/.claude/projects/-p/memory";
    let args = claude_arguments(&pm("off", Some(dir)), None).unwrap();
    let allow_at = args.iter().position(|a| a == "--allowedTools").unwrap();
    assert_eq!(
        args[allow_at + 2],
        "Edit(//home/u/.claude/projects/-p/memory/**)"
    );
    let s = settings(&args);
    assert_eq!(s["disableAllHooks"], false);
    let hooks = s["hooks"].as_object().unwrap();
    assert_eq!(hooks.len(), 1, "hooks = off installs no inbox hook");
    let hook = &hooks["PreToolUse"][0];
    assert_eq!(hook["matcher"], "Write|Edit|MultiEdit");
    assert_eq!(
        hook["hooks"][0]["command"],
        format!("cstan guard-write '{dir}' || exit 2")
    );
}

#[test]
fn guard_with_inherit_keeps_the_inbox_hook() {
    let args = claude_arguments(&pm("inherit", Some("/m/memory")), None).unwrap();
    let s = settings(&args);
    assert!(s["hooks"]["PostToolUse"].is_array() && s["hooks"]["PreToolUse"].is_array());
}

#[test]
fn without_a_guard_hooks_off_still_disables_everything() {
    let args = claude_arguments(&pm("off", None), None).unwrap();
    let s = settings(&args);
    assert_eq!(s["disableAllHooks"], true);
    assert!(s.get("hooks").is_none());
    assert!(!args.iter().any(|a| a.starts_with("Edit(")));
}

#[test]
fn unusable_guard_folders_are_refused() {
    for bad in [
        "relative/memory",
        "/a/../b",
        "/a/it's",
        "/a/*",
        "/a/$HOME",
        "/a/b\nc",
        "/a/{x}",
        "/a/!x",
    ] {
        assert!(!guard_path_is_usable(bad), "{bad:?}");
        assert!(
            claude_arguments(&pm("off", Some(bad)), None).is_err(),
            "{bad:?}"
        );
    }
    assert!(guard_path_is_usable("/home/a b/.claude/projects/-x/memory"));
}

fn developer(dirs: &[&str]) -> ClaudeRoleSettings {
    let mut role = pm("inherit", None);
    role.write_dirs = dirs.iter().map(|d| d.to_string()).collect();
    role
}

#[test]
fn write_dirs_allow_each_folder_and_install_the_dirs_hook() {
    let args = claude_arguments(&developer(&["/w/docs/a", "/w/docs/b"]), None).unwrap();
    assert!(args.contains(&"Edit(//w/docs/a/**)".to_string()));
    assert!(args.contains(&"Edit(//w/docs/b/**)".to_string()));
    let deny_at = args.iter().position(|a| a == "--disallowedTools").unwrap();
    let deny = &args[deny_at + 1..deny_at + 4];
    assert_eq!(deny, ["NotebookEdit", "Agent", "Task"]);
    assert!(!args.contains(&"Write".to_string()) && !args.contains(&"Edit".to_string()));
    let s = settings(&args);
    assert_eq!(s["disableAllHooks"], false);
    let hook = &s["hooks"]["PreToolUse"][0];
    assert_eq!(hook["matcher"], "Write|Edit|MultiEdit");
    assert_eq!(
        hook["hooks"][0]["command"],
        "cstan guard-write --dirs '/w/docs/a' '/w/docs/b' || exit 2"
    );
}

#[test]
fn both_guards_are_refused() {
    let mut role = developer(&["/w/docs"]);
    role.write_guard = Some("/m/memory".into());
    let error = claude_arguments(&role, None).unwrap_err();
    assert!(format!("{error:?}").contains("either the memory guard or write folders"));
}

#[test]
fn an_unusable_write_dir_is_refused() {
    for bad in ["docs", "/a/../b", "/a/it's", "/a/*"] {
        let error = claude_arguments(&developer(&[bad]), None).unwrap_err();
        assert!(format!("{error:?}").contains("a write folder is not a usable path"));
    }
}

#[test]
fn empty_write_dirs_change_nothing() {
    let args = claude_arguments(&developer(&[]), None).unwrap();
    assert_eq!(args, claude_arguments(&pm("inherit", None), None).unwrap());
}
