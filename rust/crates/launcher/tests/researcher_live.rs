//! Opt-in live check of the Researcher role: a real Claude Code, a real Herdr session, the real network and
//! `npx @playwright/mcp` (replaces test/researcher-live.test.ts). It runs only with `CSTAN_LIVE=1` and
//! `CAPSTAN_LIVE_RESEARCHER=1` and is not part of any default pass. It checks the config, the spawn with the playwright MCP
//! server connected, and a curl pipeline that runs with no permission prompt. Reddit may answer with a login redirect, 403
//! or 429; that still proves the curl path.
//!
//! The agents run the real `claude` on the harness's scratch HOME, to which the operator's Claude sign-in and the
//! Playwright browsers are linked, never copied. Needs herdr, claude, git, jq and npx; without them (or the flags) it says
//! `SKIPPED LOUDLY` and passes.
//!
//! Run by hand after `cargo build -p cstan-front`:
//! `CSTAN_LIVE=1 CAPSTAN_LIVE_RESEARCHER=1 cargo test -p capstan-launcher --test researcher_live -- --ignored`

#[path = "../../herdr/tests/common/live_env.rs"]
mod live_env;

use live_env::{text, until, Live, Options};
use serde_json::Value;
use std::process::{Command, Stdio};

const BLOCKS: [&str; 2] = ["# [researcher]", "# [roles.researcher]"];

/// Uncomments the researcher blocks of the starter template: a header line and the key lines that follow it up to the next
/// blank line.
fn enable_researcher(template: &str) -> String {
    let mut on = false;
    let lines: Vec<String> = template
        .split('\n')
        .map(|line| {
            if BLOCKS.contains(&line) {
                on = true;
            } else if on && !line.starts_with("# ") {
                on = false;
            }
            if on {
                line[2..].to_string()
            } else {
                line.to_string()
            }
        })
        .collect();
    format!("herdr_session = \"{{session}}\"\n{}", lines.join("\n"))
}

fn have(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn wait_screen(live: &Live, pane: &str, what: &str, seconds: u64, done: impl Fn(&str) -> bool) {
    until(what, seconds, || done(&live.screen(pane)).then_some(()));
}

/// Types `line` into the pane and presses Enter once the text is on the screen.
fn say(live: &Live, pane: &str, line: &str) {
    live.herdr(&["pane", "send-text", pane, line]).unwrap();
    let head: String = line.chars().take(24).collect();
    wait_screen(live, pane, "the typed text", 30, |s| s.contains(&head));
    live.herdr(&["pane", "send-keys", pane, "enter"]).unwrap();
}

#[test]
#[ignore = "drives a real Herdr server, a real Claude Code and the network; run by hand: CSTAN_LIVE=1 CAPSTAN_LIVE_RESEARCHER=1 cargo test -p capstan-launcher --test researcher_live -- --ignored"]
fn a_spawned_researcher_has_the_playwright_mcp_connected_and_runs_a_read_only_curl_pipeline_with_no_permission_prompt(
) {
    if std::env::var("CAPSTAN_LIVE_RESEARCHER").as_deref() != Ok("1") {
        eprintln!("SKIPPED LOUDLY: live researcher test: set CAPSTAN_LIVE_RESEARCHER=1 to run");
        return;
    }
    for program in ["jq", "npx"] {
        if !have(program) {
            eprintln!("SKIPPED LOUDLY: live researcher test: {program} is not available");
            return;
        }
    }
    let Some(mut live) = Live::start(&Options {
        linked_login: true,
        config: Some(enable_researcher(capstan_config::starter::STARTER_CONFIG)),
        ..Options::default()
    }) else {
        return;
    };
    // The starter's designer prompt file, which `cstan init` writes beside capstan.toml.
    std::fs::create_dir_all(live.repo.join("roles")).unwrap();
    std::fs::write(
        live.repo
            .join(capstan_config::starter::DESIGNER_PROMPT_PATH),
        capstan_config::starter::DESIGNER_PROMPT,
    )
    .unwrap();

    // Step 1: the starter template with the researcher blocks uncommented passes the config check.
    let check = live.cstan(&["config", "check"]);
    assert!(check.status.success(), "{}", text(&check));
    let resolved: Value = serde_json::from_slice(&check.stdout).unwrap();
    assert_eq!(resolved["researcher"]["enabled"], true, "{resolved}");
    let servers: Vec<&str> = resolved["mcpServers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(servers, ["playwright"]);

    // Step 2: spawn the researcher; Claude Code lists the playwright server as connected.
    live.start_daemon();
    let launched = live.launch_pm();
    assert_eq!(launched["state"], "blocked", "{launched}");
    let pm = launched["paneId"].as_str().unwrap().to_string();
    wait_screen(&live, &pm, "the PM dialog", 120, |s| {
        s.contains("trust this folder")
    });
    live.herdr(&["pane", "send-keys", &pm, "down"]).unwrap();
    live.herdr(&["pane", "send-keys", &pm, "enter"]).unwrap();
    let spawned = live.cstan_ok(&["spawn", "researcher"]);
    assert_eq!(spawned["state"], "started", "{spawned}");
    let pane = spawned["paneId"].as_str().unwrap().to_string();
    wait_screen(&live, &pane, "the researcher prompt", 120, |s| {
        s.contains("manual mode on")
    });
    say(&live, &pane, "/mcp");
    wait_screen(
        &live,
        &pane,
        "the playwright MCP server to show as connected (the first run downloads it through npx)",
        300,
        |s| s.contains("✔ playwright"),
    );
    live.herdr(&["pane", "send-keys", &pane, "escape"]).unwrap();
    wait_screen(&live, &pane, "the MCP panel to close", 30, |s| {
        !s.contains("✔ playwright")
    });

    // Step 3: the curl pipeline runs with no permission prompt. A login redirect, 403 or 429 from Reddit leaves empty output
    // and still counts.
    say(
        &live,
        &pane,
        "Run exactly this one shell command and show its output: curl -sS -A 'capstan-researcher/1.0 (research bot; contact: project owner)' 'https://old.reddit.com/r/ClaudeAI/search.json?q=design&restrict_sr=1&limit=3' | jq '.data.children[].data.title'",
    );
    let verdicts = [
        "Ran 1 shell command",
        "Permission to use",
        "Do you want to proceed",
    ];
    wait_screen(&live, &pane, "the curl command to run", 120, |s| {
        verdicts.iter().any(|v| s.contains(v))
    });
    let screen = live.screen(&pane);
    assert!(!screen.contains("Do you want to proceed"), "{screen}");
    assert!(!screen.contains("Permission to use Bash"), "{screen}");
    assert!(screen.contains("Ran 1 shell command"), "{screen}");

    // Step 4: a browser visit works and leaves no .playwright-mcp directory in the worktree.
    let worktree = std::path::PathBuf::from(spawned["worktreePath"].as_str().unwrap());
    say(
        &live,
        &pane,
        "Use browser_navigate to load https://news.ycombinator.com, then browser_snapshot, and name the first story.",
    );
    wait_screen(&live, &pane, "the browser visit", 120, |s| {
        s.contains("Called playwright 2 times")
    });
    assert!(
        !worktree.join(".playwright-mcp").exists(),
        "no .playwright-mcp directory in the worktree"
    );

    live.stop_daemon();
    live.close();
}
