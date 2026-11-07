//! A project whose `capstan.toml` has checked keys and no trust record, from the upgrade to a controller that serves
//! the trusted file: what is refused, what keeps serving and what `cstan trust` changes. Real processes, launch off.
mod support;

use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::{json, Value};
use support::*;

fn cstan(args: &[&str], cwd: &Path, env: &[(&str, &str)]) -> std::process::Output {
    Command::new(FRONT)
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .envs(env.iter().copied())
        .stdin(Stdio::null())
        .output()
        .expect("cstan runs")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn project(label: &str) -> Scratch {
    let scratch = Scratch::new(label, 0);
    let root = scratch.text();
    let layout = json!({
        "dirs": ["proj/.capstan/state"],
        "files": [],
        "contents": {
            "proj/.capstan/operator.key": "fixture-operator-key-0123456789abcdefghijklmnopqrstuvwxyz\n",
            "proj/.capstan/project.json": format!("{{\n  \"schemaVersion\": 1,\n  \"projectId\": \"pfixture\",\n  \"name\": \"proj\",\n  \"stateDirectory\": \"{root}/proj/.capstan/state\",\n  \"maxSlices\": 4,\n  \"maxRunMs\": 3600000,\n  \"maxDispatches\": 16\n}}\n"),
        },
        "modes": {"proj/.capstan": "0700", "proj/.capstan/state": "0700"},
        "git": [{"dir": "proj", "commit": true}],
    });
    make_layout(&layout, &scratch.0, &root);
    scratch
}

/// N of the line `trust: needed (N keys) - run cstan trust`; 0 when there is none.
fn status_keys(out: &str) -> usize {
    out.lines()
        .find_map(|line| line.strip_prefix("trust: needed ("))
        .and_then(|rest| rest.strip_suffix(" keys) - run cstan trust"))
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

fn nothing_spawned(project: &Path) {
    assert!(!project.join(".capstan/state/control.sock").exists());
    assert!(!project.join(".capstan/state/daemon.pid").exists());
}

#[test]
fn the_upgrade_path_from_no_record_to_a_trusted_controller() {
    let scratch = project("upgrade-path");
    let project = scratch.0.join("proj");
    let home = scratch.text();
    let trust_dir = trust_dir_of(&scratch.0).to_string_lossy().into_owned();
    let env = [
        ("PATH", HOST_PATH),
        ("HOME", home.as_str()),
        ("CAPSTAN_LAUNCH", "off"),
        ("CAPSTAN_TRUST_DIR", trust_dir.as_str()),
    ];
    write_config_untrusted(&project, capstan_config::starter::STARTER_CONFIG);

    // Nothing is trusted: a start, a routed command and a watch refuse and spawn nothing.
    let started = cstan(&["start"], &project, &env);
    assert_eq!(started.status.code(), Some(3), "{}", text(&started.stderr));
    assert!(
        text(&started.stderr).contains("cstan trust"),
        "{}",
        text(&started.stderr)
    );
    nothing_spawned(&project);
    let routed = cstan(&["ping"], &project, &env);
    assert_eq!(routed.status.code(), Some(3), "{}", text(&routed.stderr));
    nothing_spawned(&project);
    let watch = cstan(&["status", "--watch"], &project, &env);
    assert_eq!(watch.status.code(), Some(3), "{}", text(&watch.stderr));
    nothing_spawned(&project);

    // Status says what is needed, in text and as a member.
    let status = cstan(&["status"], &project, &env);
    let keys = status_keys(&text(&status.stdout));
    assert!(keys > 0, "{}", text(&status.stdout));
    let status = cstan(&["status", "--json"], &project, &env);
    let json: Value = serde_json::from_str(&text(&status.stdout)).unwrap();
    assert_eq!(json["trust"], json!({"state": "needed", "keys": keys}));

    // Trusting it lets a start through, and the controller loads the trusted file.
    let trusted = cstan(&["trust"], &project, &env);
    assert_eq!(trusted.status.code(), Some(0), "{}", text(&trusted.stderr));
    let started = cstan(&["start", "--json"], &project, &env);
    assert_eq!(started.status.code(), Some(0), "{}", text(&started.stderr));
    assert!(
        text(&started.stdout).contains("\"started\": true"),
        "{}",
        text(&started.stdout)
    );
    let status = cstan(&["status", "--json"], &project, &env);
    let json: Value = serde_json::from_str(&text(&status.stdout)).unwrap();
    assert!(json.get("trust").is_none(), "{json}");

    // A change pulled in later: the running controller keeps answering; a restart is refused and leaves it alone.
    let pulled =
        capstan_config::starter::STARTER_CONFIG.replace("max_workers = 3", "max_workers = 4");
    assert_ne!(pulled, capstan_config::starter::STARTER_CONFIG);
    write_config_untrusted(&project, &pulled);
    let ping = cstan(&["ping"], &project, &env);
    assert_eq!(ping.status.code(), Some(0), "{}", text(&ping.stderr));
    let restart = cstan(&["restart"], &project, &env);
    assert_eq!(restart.status.code(), Some(3), "{}", text(&restart.stderr));
    assert!(
        text(&restart.stderr).contains("cstan trust"),
        "{}",
        text(&restart.stderr)
    );
    let ping = cstan(&["ping"], &project, &env);
    assert_eq!(ping.status.code(), Some(0), "{}", text(&ping.stderr));

    let _ = cstan(&["stop"], &project, &env);
    kill_daemons_of(&project);
}
