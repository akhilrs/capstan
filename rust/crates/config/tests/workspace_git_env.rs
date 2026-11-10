//! The git checks of `validate_workspace` keep the user's global git configuration reachable (HOME and
//! XDG_CONFIG_HOME), so they agree with the cstan checks that inherit the environment. This file holds one test
//! because it changes the process environment.
use std::os::unix::fs::PermissionsExt;

#[test]
fn git_keeps_home_and_xdg_config_home() {
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let seen = tmp.path().join("seen");
    let fake = bin.join("git");
    std::fs::write(
        &fake,
        format!(
            "#!/bin/sh\nprintf '%s|%s' \"$HOME\" \"$XDG_CONFIG_HOME\" > '{}'\nexit 1\n",
            seen.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::env::set_var("PATH", &bin);
    std::env::set_var("HOME", "/the/home");
    std::env::set_var("XDG_CONFIG_HOME", "/the/xdg");
    assert!(!capstan_config::is_work_tree_root(tmp.path()));
    assert_eq!(
        std::fs::read_to_string(&seen).unwrap(),
        "/the/home|/the/xdg"
    );
}
