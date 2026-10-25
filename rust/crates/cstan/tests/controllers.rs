//! `cstan controllers` over fake process data and fake controller answers: no real process is looked at or signalled.
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use cstan_front::controllers::{
    cpu_percent, format_bytes, format_uptime, is_controller_command, parse_cpu_time, parse_elapsed,
    parse_lsof_cwd, parse_ps_line, render_table, rows, rows_json, shorten_home, About, Probe, Proc,
    Reply, State,
};

struct Fake {
    ledgers: Vec<PathBuf>,
    replies: HashMap<PathBuf, Reply>,
}

impl Probe for Fake {
    fn ledger_exists(&self, project: &Path) -> bool {
        self.ledgers.iter().any(|p| p == project)
    }
    fn ask(&self, project: &Path) -> Reply {
        self.replies.get(project).cloned().unwrap_or(Reply::Silent)
    }
}

fn proc(pid: u32, project: &str) -> Proc {
    Proc {
        pid,
        project: Some(PathBuf::from(project)),
        project_gone: false,
        started_at: Some(1_000),
        rss_bytes: Some(50 * 1024 * 1024),
        cpu_percent: Some(1.25),
    }
}

fn answer(pid: u32, version: Option<&str>, agents: Option<u64>) -> Reply {
    Reply::Answered(About {
        pid: Some(pid),
        version: version.map(String::from),
        active_agents: agents,
    })
}

fn fake() -> Fake {
    Fake {
        ledgers: ["/home/u/a", "/home/u/b", "/home/u/c", "/home/u/d"]
            .map(PathBuf::from)
            .to_vec(),
        replies: HashMap::from([
            (
                PathBuf::from("/home/u/a"),
                answer(10, Some("0.10.0"), Some(3)),
            ),
            (PathBuf::from("/home/u/b"), Reply::Silent),
            // Another process now serves project c.
            (
                PathBuf::from("/home/u/c"),
                answer(99, Some("0.10.0"), Some(5)),
            ),
            // An older build: it answers ping with its pid only.
            (PathBuf::from("/home/u/d"), answer(40, None, None)),
        ]),
    }
}

#[test]
fn a_controller_that_answers_is_serving() {
    let found = rows(&[proc(10, "/home/u/a")], 1_090, &fake());
    let row = &found[0];
    assert_eq!(row.state, State::Serving);
    assert_eq!(row.note, None);
    assert_eq!(row.version.as_deref(), Some("0.10.0"));
    assert_eq!(row.active_agents, Some(3));
    assert_eq!(row.uptime_seconds, Some(90));
}

#[test]
fn a_controller_that_stays_silent_is_not_answering() {
    let found = rows(&[proc(20, "/home/u/b")], 1_090, &fake());
    assert_eq!(found[0].state, State::NotAnswering);
    assert_eq!(found[0].version, None);
    assert_eq!(found[0].active_agents, None);
}

#[test]
fn a_controller_without_a_key_or_with_a_refused_key_says_why() {
    let mut fake = fake();
    fake.replies
        .insert(PathBuf::from("/home/u/a"), Reply::NoKey);
    fake.replies
        .insert(PathBuf::from("/home/u/b"), Reply::Refused);
    let found = rows(
        &[proc(10, "/home/u/a"), proc(20, "/home/u/b")],
        1_090,
        &fake,
    );
    assert_eq!(found[0].note.as_deref(), Some("(no operator key)"));
    assert_eq!(found[1].note.as_deref(), Some("(key refused)"));
    assert!(found.iter().all(|row| row.state == State::NotAnswering));
}

#[test]
fn a_controller_whose_ledger_was_deleted_is_orphaned() {
    let mut fake = fake();
    fake.ledgers.retain(|p| p != Path::new("/home/u/a"));
    let found = rows(&[proc(10, "/home/u/a")], 1_090, &fake);
    assert_eq!(found[0].state, State::Orphaned);
    assert_eq!(found[0].note.as_deref(), Some("(ledger deleted)"));
}

#[test]
fn a_controller_in_a_deleted_project_is_orphaned() {
    let mut gone = proc(10, "/home/u/a");
    gone.project_gone = true;
    let found = rows(&[gone], 1_090, &fake());
    assert_eq!(found[0].state, State::Orphaned);
    assert_eq!(found[0].note.as_deref(), Some("(project deleted)"));
}

#[test]
fn a_controller_another_process_has_replaced_is_orphaned() {
    let found = rows(&[proc(30, "/home/u/c")], 1_090, &fake());
    let row = &found[0];
    assert_eq!(row.state, State::Orphaned);
    assert_eq!(row.note.as_deref(), Some("(superseded by 99)"));
    // What the other process said is not this process's.
    assert_eq!(row.version, None);
    assert_eq!(row.active_agents, None);
}

#[test]
fn an_older_build_serves_without_a_version() {
    let found = rows(&[proc(40, "/home/u/d")], 1_090, &fake());
    assert_eq!(found[0].state, State::Serving);
    assert_eq!(found[0].version, None);
    assert_eq!(found[0].active_agents, None);
    let table = render_table(&found, None);
    let line = table.lines().nth(1).unwrap();
    assert!(line.contains("serving  ?  "), "{line}");
}

#[test]
fn the_table_has_the_columns_and_shortens_home() {
    let found = rows(
        &[
            proc(30, "/home/u/c"),
            proc(10, "/home/u/a"),
            proc(20, "/home/u/b"),
        ],
        1_090,
        &fake(),
    );
    let table = render_table(&found, Some(Path::new("/home/u")));
    let lines: Vec<&str> = table.lines().collect();
    assert_eq!(lines.len(), 4);
    for column in [
        "PID", "STATE", "VERSION", "UPTIME", "CPU", "MEM", "AGENTS", "PROJECT",
    ] {
        assert!(lines[0].contains(column), "{column}");
    }
    assert!(lines[1].starts_with("10 "), "sorted by pid: {}", lines[1]);
    assert!(lines[1].contains("serving"));
    assert!(lines[1].contains("0.10.0"));
    assert!(lines[1].contains("1m30s"));
    assert!(lines[1].contains("1.2%") || lines[1].contains("1.3%"));
    assert!(lines[1].contains("50.0 MB"));
    assert!(lines[1].ends_with("~/a"), "{}", lines[1]);
    assert!(lines[2].contains("not answering"));
    assert!(lines[3].contains("orphaned (superseded by 99)"));
    assert!(lines.iter().all(|l| !l.ends_with(' ')));
}

#[test]
fn no_controllers_says_so() {
    assert_eq!(render_table(&[], None), "No controllers are running.\n");
}

#[test]
fn the_json_has_a_stable_shape_with_full_paths() {
    let found = rows(
        &[proc(10, "/home/u/a"), proc(40, "/home/u/d")],
        1_090,
        &fake(),
    );
    let json = rows_json(&found);
    assert_eq!(
        json,
        serde_json::json!([
            {
                "pid": 10,
                "state": "serving",
                "note": null,
                "version": "0.10.0",
                "uptimeSeconds": 90,
                "cpuPercent": 1.3,
                "memoryBytes": 52_428_800,
                "activeAgents": 3,
                "project": "/home/u/a"
            },
            {
                "pid": 40,
                "state": "serving",
                "note": null,
                "version": null,
                "uptimeSeconds": 90,
                "cpuPercent": 1.3,
                "memoryBytes": 52_428_800,
                "activeAgents": null,
                "project": "/home/u/d"
            }
        ])
    );
    let keys: Vec<&String> = json[0].as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        [
            "pid",
            "state",
            "note",
            "version",
            "uptimeSeconds",
            "cpuPercent",
            "memoryBytes",
            "activeAgents",
            "project"
        ]
    );
}

#[test]
fn only_the_daemon_command_lines_are_controllers() {
    assert!(is_controller_command(&["/usr/local/bin/cstan", "daemon"]));
    assert!(is_controller_command(&["cstan", "daemon"]));
    assert!(is_controller_command(&["/opt/cstan-daemon"]));
    assert!(!is_controller_command(&["cstan", "status"]));
    assert!(!is_controller_command(&["cstan", "daemon", "x"]));
    assert!(!is_controller_command(&["vim", "daemon"]));
    assert!(!is_controller_command::<&str>(&[]));
}

#[test]
fn numbers_read_and_print_as_expected() {
    assert_eq!(
        cpu_percent(1.0, 1.02, 0.2).map(|c| (c * 10.0).round()),
        Some(100.0)
    );
    assert_eq!(cpu_percent(2.0, 1.0, 0.2), None);
    assert_eq!(format_uptime(42), "42s");
    assert_eq!(format_uptime(7 * 60 + 3), "7m03s");
    assert_eq!(format_uptime(5 * 3600 + 12 * 60), "5h12m");
    assert_eq!(format_uptime(3 * 86_400 + 4 * 3600), "3d04h");
    assert_eq!(format_bytes(512), "512 B");
    assert_eq!(format_bytes(812 * 1024), "812 KB");
    assert_eq!(format_bytes(1_363_148_800), "1.3 GB");
    assert_eq!(
        shorten_home(Path::new("/home/u"), Some(Path::new("/home/u"))),
        "~"
    );
    assert_eq!(
        shorten_home(Path::new("/home/ux/a"), Some(Path::new("/home/u"))),
        "/home/ux/a"
    );
    assert_eq!(shorten_home(Path::new("/srv/a"), None), "/srv/a");
}

#[test]
fn the_ps_and_lsof_text_parses() {
    assert_eq!(parse_elapsed("05:07"), Some(307));
    assert_eq!(parse_elapsed("1-02:03:04"), Some(86_400 + 7_384));
    assert_eq!(parse_elapsed("x"), None);
    assert_eq!(parse_cpu_time("0:01.50"), Some(1.5));
    assert_eq!(parse_cpu_time("1:00:00.00"), Some(3_600.0));
    let (pid, uid, started, rss, cpu, words) = parse_ps_line(
        "  812 501 02:00 2048 0:03.00 /usr/local/bin/cstan daemon",
        10_000,
    )
    .unwrap();
    assert_eq!(
        (pid, uid, started, rss, cpu),
        (812, 501, 9_880, 2_097_152, 3.0)
    );
    assert!(is_controller_command(&words));
    assert_eq!(parse_ps_line("garbage", 0), None);
    assert_eq!(
        parse_lsof_cwd("p812\nfcwd\nn/Users/u/proj\n"),
        Some(PathBuf::from("/Users/u/proj"))
    );
}
