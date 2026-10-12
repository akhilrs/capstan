//! The text `cstan verify` and `cstan status` print for the verify run records of the daemon.
use capstan_wire::js::{self, Value};
use cstan_front::{compact, render};

fn parse(json: &str) -> Value {
    js::parse(json).expect("fixture parses")
}

fn run(state: &str, extra: &str) -> Value {
    parse(&format!(
        r#"{{"runId":"vr-1","sequence":1,"subjectKind":"report","subjectId":"rep-1","commitSha":"0123456789abcdef0123456789abcdef01234567",
        "commands":["cargo build","cargo test"],"commandsSha":"x","trigger":"manual","requestedBy":"pm","state":"{state}",
        "queuedAt":"2026-10-13T10:00:00.000Z","startedAt":null,"finishedAt":null,"exitCode":null,"failedStep":null,
        "durationMs":null,"outputTail":"","outputTruncated":false,"logPath":null{extra}}}"#
    ))
}

const NOW: f64 = 1_791_885_660_000.0; // 2026-10-13T10:01:00Z

fn show(value: &Value) -> String {
    render::verify_lines("show", Some(value), NOW).join("\n")
}

#[test]
fn show_renders_every_state() {
    for state in [
        "queued",
        "running",
        "passed",
        "failed",
        "timeout",
        "error",
        "interrupted",
        "cancelled",
    ] {
        let out = show(&run(state, ""));
        assert!(
            out.starts_with(&format!("verify run vr-1: {state}\n")),
            "{out}"
        );
        assert!(
            out.contains("commit: 0123456789abcdef0123456789abcdef01234567"),
            "{out}"
        );
        assert!(out.contains("  1. cargo build\n  2. cargo test"), "{out}");
    }
}

#[test]
fn show_a_failed_run_names_exit_code_step_duration_and_log() {
    let out = show(&run(
        "failed",
        r#","exitCode":101,"failedStep":2,"durationMs":842000,"outputTail":"error: boom\n","logPath":"/p/log.txt""#,
    ));
    assert!(out.contains("exit code: 101"), "{out}");
    assert!(out.contains("failed step: 2 (cargo test)"), "{out}");
    assert!(out.contains("duration: 14m02s"), "{out}");
    assert!(out.contains("log: /p/log.txt"), "{out}");
    assert!(out.contains("output:\n  error: boom"), "{out}");
}

#[test]
fn a_long_tail_is_cut_to_the_last_forty_lines() {
    let tail: String = (1..=100).map(|n| format!("line {n}\\n")).collect();
    let out = show(&run(
        "failed",
        &format!(
            r#","failedStep":1,"outputTruncated":true,"outputTail":"{tail}","logPath":"/p/log.txt""#
        ),
    ));
    assert!(!out.contains("line 60\n"), "{out}");
    assert!(out.contains("  line 61\n"), "{out}");
    assert!(out.contains("line 100"), "{out}");
    assert!(
        out.contains("output (last 40 lines, earlier output cut):"),
        "{out}"
    );
    assert!(out.contains("(full output: /p/log.txt)"), "{out}");
}

#[test]
fn queue_list_and_cancel_answers() {
    let queued = render::verify_lines("", Some(&run("queued", "")), NOW);
    assert_eq!(
        queued,
        ["verify run vr-1 queued at 0123456789ab; cstan verify show vr-1"]
    );
    let again = render::verify_lines("", Some(&run("running", r#","existing":true"#)), NOW);
    assert!(
        again[0].starts_with("verify run vr-1 already running at 0123456789ab"),
        "{again:?}"
    );
    let list = parse(&format!(
        r#"{{"runs":[{}]}}"#,
        js::stringify(&run("passed", r#","durationMs":62000"#), 0).to_utf8_lossy()
    ));
    assert_eq!(
        render::verify_lines("list", Some(&list), NOW),
        ["vr-1 passed report rep-1 @0123456789ab 1m02s"]
    );
    assert_eq!(
        render::verify_lines("list", Some(&parse(r#"{"runs":[]}"#)), NOW),
        ["no verify runs"]
    );
    assert_eq!(
        render::verify_lines(
            "cancel",
            Some(&parse(r#"{"runId":"vr-1","state":"cancelled"}"#)),
            NOW
        ),
        ["verify run vr-1: cancelled"]
    );
}

#[test]
fn status_verify_section() {
    let status = parse(&format!(
        r#"{{"verify":{{"active":[{}],"recent":[{}]}}}}"#,
        js::stringify(
            &run("running", r#","startedAt":"2026-10-13T09:58:00.000Z""#),
            0
        )
        .to_utf8_lossy(),
        js::stringify(&run("failed", r#","failedStep":2,"durationMs":5000"#), 0).to_utf8_lossy(),
    ));
    let lines = render::verify_status_lines(Some(&status), NOW);
    assert_eq!(
        lines,
        [
            "verify active:",
            "  vr-1 running report rep-1 @0123456789ab started 3m ago",
            "verify recent:",
            "  vr-1 failed report rep-1 @0123456789ab 5s failed step 2",
        ]
    );
    assert!(render::verify_status_lines(Some(&parse(r#"{"agents":[]}"#)), NOW).is_empty());
    assert!(render::verify_status_lines(
        Some(&parse(r#"{"verify":{"active":[],"recent":[]}}"#)),
        NOW
    )
    .is_empty());
}

#[test]
fn plan_rows_summarise_the_latest_gate() {
    let cases = [
        ("null", "no gate"),
        (
            r#"{"runId":"v","state":"passed","durationMs":842000,"failedStep":null}"#,
            "gate passed 14m02s",
        ),
        (
            r#"{"runId":"v","state":"failed","durationMs":1,"failedStep":2}"#,
            "gate failed step 2",
        ),
        (
            r#"{"runId":"v","state":"running","durationMs":null}"#,
            "gate running",
        ),
        (
            r#"{"runId":"v","state":"queued","durationMs":null}"#,
            "gate queued",
        ),
        (
            r#"{"runId":"v","state":"timeout","durationMs":9}"#,
            "gate timed out",
        ),
        (
            r#"{"runId":"v","state":"cancelled","durationMs":null}"#,
            "gate cancelled",
        ),
    ];
    for (verify, summary) in cases {
        assert_eq!(render::verify_summary(Some(&parse(verify))), summary);
        let plan = parse(&format!(
            r#"{{"plan":{{"planId":"plan-1","tier":"full","state":"active","title":"T"}},"packages":[{{"packageId":"p1","type":"code","estimateHours":2,"dependsOn":[],"assignee":"dev","progress":"working","verify":{verify}}}]}}"#
        ));
        let lines = compact::plan_brief(&plan).unwrap();
        assert!(lines[1].contains(&format!(" {summary}")), "{lines:?}");
    }
    assert_eq!(render::verify_summary(None), "no gate");
}

#[test]
fn rows_and_status_without_verify_are_unchanged() {
    let plan = parse(
        r#"{"plan":{"planId":"plan-1","tier":"full","state":"active","title":"T"},"packages":[{"packageId":"p1","type":"code","estimateHours":2,"dependsOn":[],"assignee":"dev","progress":"working"}]}"#,
    );
    assert_eq!(
        compact::plan_brief(&plan).unwrap()[1],
        "  p1 code 2h deps - dev working"
    );
}
