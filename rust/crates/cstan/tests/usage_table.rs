//! The command table behind `cstan help`, `cstan help <command>`, the usage hints and the man page: every command of the old
//! usage line is still there, the output fits 80 columns, and the man page lists what the help lists.
mod support;

use cstan_front::usage::{self, ENTRIES, WIDTH};
use cstan_front::VERSION;
use support::{node_usage, run_once, Settings};

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The old one-line usage as its commands: split at ` | cstan `, without the leading `usage: cstan `.
fn old_commands() -> Vec<String> {
    let line = node_usage().strip_prefix("usage: cstan ").unwrap();
    line.split(" | cstan ").map(str::to_string).collect()
}

fn run(args: &[&str]) -> (String, String, i32) {
    let settings = Settings {
        env: vec![],
        cwd: std::env::temp_dir(),
        now_ms: 0.0,
        terminal: Some(false),
    };
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    let observed = run_once(&args, &settings);
    (observed.stdout, observed.stderr, observed.exit.unwrap())
}

#[test]
fn the_table_keeps_every_command_of_the_old_usage_line() {
    let old = old_commands();
    assert!(old.len() >= 54, "{} commands parsed", old.len());
    for command in &old {
        assert!(
            ENTRIES
                .iter()
                .any(|e| e.syntax.starts_with(command.as_str())),
            "no table entry for `cstan {command}`"
        );
    }
    // The bare commands got their arguments.
    for (bare, filled) in [
        ("ack", "ack <message-id>...|--all-read"),
        ("report", "report <commit> <summary>"),
        ("resolve", "resolve <message-id> retry|skip|cancel [<note>]"),
    ] {
        assert!(ENTRIES.iter().any(|e| e.syntax == filled), "{bare}");
    }
}

#[test]
fn help_lists_every_command_in_groups() {
    let (out, err, exit) = run(&["help"]);
    assert_eq!((err.as_str(), exit), ("", 0));
    assert!(out.starts_with("cstan - "), "{out}");
    assert!(out.contains("\nusage: cstan <command> [args]\n"));
    let flat = collapse(&out);
    for entry in ENTRIES {
        assert!(
            flat.contains(&collapse(entry.syntax)),
            "{} missing",
            entry.syntax
        );
        assert!(
            flat.contains(&collapse(entry.about)),
            "{} description missing",
            entry.syntax
        );
    }
    for group in [
        "Setup & daemon",
        "Run control",
        "Status & inspection",
        "Messaging",
        "Workers",
        "Review & integration",
        "Plans",
        "Nexora links",
        "Supervision",
        "Prompt relay",
        "Operator",
    ] {
        assert!(out.contains(&format!("\n{group}\n")), "{group} missing");
    }
    assert!(out.trim_end().ends_with("`man cstan` for the full manual."));
    assert!(out.contains("cstan help <command>"));
    for alias in ["--help", "-h"] {
        assert_eq!(run(&[alias]).0, out);
    }
}

#[test]
fn help_lines_fit_80_columns() {
    let help = usage::help_text();
    for line in help.lines() {
        assert!(line.chars().count() <= WIDTH, "{} wide: {line}", line.len());
    }
    for entry in ENTRIES {
        let name = entry.command;
        for text in [
            usage::command_help(name).unwrap(),
            usage::hint(&[name.to_string()]),
        ] {
            for line in text.lines() {
                assert!(line.chars().count() <= WIDTH, "{name}: {line}");
            }
        }
    }
}

#[test]
fn every_command_description_stays_in_one_line_entries() {
    // One command per entry: no syntax holds a line break, and syntaxes are unique.
    for (at, entry) in ENTRIES.iter().enumerate() {
        assert!(!entry.syntax.contains('\n'));
        assert!(
            entry.syntax.starts_with(entry.command),
            "{} is not under {}",
            entry.syntax,
            entry.command
        );
        assert!(!ENTRIES[..at].iter().any(|e| e.syntax == entry.syntax));
    }
}

#[test]
fn help_for_a_command_shows_its_syntax_and_description() {
    let (out, err, exit) = run(&["help", "plan"]);
    assert_eq!((err.as_str(), exit), ("", 0));
    for entry in ENTRIES.iter().filter(|e| e.command == "plan") {
        assert!(collapse(&out).contains(&collapse(&format!("usage: cstan {}", entry.syntax))));
        assert!(collapse(&out).contains(&collapse(entry.about)));
    }
    assert!(!out.contains("cstan op "), "only the plan group");
    let (flag_out, _, flag_exit) = run(&["plan", "--help"]);
    assert_eq!((flag_out, flag_exit), (out, 0));
    let (single, _, _) = run(&["help", "ack"]);
    assert!(
        single.starts_with("usage: cstan ack <message-id>...|--all-read\n"),
        "{single}"
    );
}

#[test]
fn an_unknown_command_gets_a_short_error_and_a_pointer_to_help() {
    let (out, err, exit) = run(&["help", "frobnicate"]);
    assert_eq!(out, "");
    assert_eq!(exit, 2);
    assert_eq!(
        err,
        "cstan: unknown command: frobnicate\nrun `cstan help` for the list of commands\n"
    );
    let (_, err, exit) = run(&["frobnicate"]);
    assert_eq!(exit, 2);
    assert!(err.contains("unknown command: frobnicate") && err.contains("run `cstan help`"));
    let (_, err, _) = run(&["--bogus"]);
    assert!(err.starts_with("cstan: unknown option: --bogus\n"), "{err}");
}

#[test]
fn a_usage_error_prints_the_syntax_of_that_command_only() {
    let (out, err, exit) = run(&["config", "bogus"]);
    assert_eq!((out.as_str(), exit), ("", 2));
    assert_eq!(
        err,
        "usage: cstan config check\n       cstan config sync\nrun `cstan help config` for details, or `cstan help` for all commands\n"
    );
    let (_, err, exit) = run(&["ping", "extra"]);
    assert_eq!(exit, 2);
    assert!(
        err.starts_with("usage: cstan ping\n") && err.lines().count() == 2,
        "{err}"
    );
    let (_, err, exit) = run(&[]);
    assert_eq!(exit, 2);
    assert_eq!(
        err,
        "usage: cstan <command> [args]\nrun `cstan help` for the list of commands\n"
    );
    for (_, err, _) in [run(&["status", "x"]), run(&["inspect"])] {
        assert!(err.len() < 400, "short hint, got {err}");
        assert!(!err.contains("cstan op "));
    }
}

/// The text of `.B cstan <syntax>` lines of the man page, unescaped.
fn man_commands(page: &str) -> Vec<String> {
    page.lines()
        .filter_map(|l| l.strip_prefix(".B cstan "))
        .map(|l| l.replace("\\-", "-").replace("\\e", "\\"))
        .collect()
}

#[test]
fn the_man_page_lists_every_command_of_the_table() {
    let (page, err, exit) = run(&["help", "--man"]);
    assert_eq!((err.as_str(), exit), ("", 0));
    assert_eq!(page, usage::man_page(VERSION));
    let listed = man_commands(&page);
    for entry in ENTRIES {
        assert!(
            listed.iter().any(|l| l == entry.syntax),
            "{} missing",
            entry.syntax
        );
        assert!(
            page.contains(&entry.about.replace('\\', "\\e").replace('-', "\\-")),
            "{} description missing",
            entry.syntax
        );
    }
    for section in [
        ".SH NAME",
        ".SH SYNOPSIS",
        ".SH DESCRIPTION",
        ".SH COMMANDS",
        ".SH FILES",
        ".SH ENVIRONMENT",
        ".SH EXIT STATUS",
        ".SH SEE ALSO",
    ] {
        assert!(page.lines().any(|l| l == section), "{section} missing");
    }
    assert!(page.contains(&format!("\"cstan {VERSION}\"")));
}

#[test]
fn the_man_page_renders_when_a_man_formatter_is_installed() {
    let Ok(mut child) = std::process::Command::new("groff")
        .args(["-man", "-Tutf8", "-ww", "-z"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    else {
        eprintln!("groff is not installed; skipping");
        return;
    };
    std::io::Write::write_all(
        &mut child.stdin.take().unwrap(),
        usage::man_page(VERSION).as_bytes(),
    )
    .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "",
        "groff warned about the page"
    );
}

#[test]
fn cleanup_is_in_the_help_the_command_help_and_the_man_page() {
    let (help, _, _) = run(&["help"]);
    for syntax in [
        "cleanup --dry-run",
        "cleanup apply <cleanup-id> [<branch>...]",
        "cleanup decline <cleanup-id>",
    ] {
        assert!(collapse(&help).contains(syntax), "{syntax}");
    }
    let (one, err, exit) = run(&["help", "cleanup"]);
    assert_eq!((err.as_str(), exit), ("", 0));
    assert_eq!(one.matches("usage: cstan cleanup").count(), 4, "{one}");
    let (page, _, _) = run(&["help", "--man"]);
    assert!(man_commands(&page)
        .iter()
        .any(|l| l == "cleanup apply <cleanup-id> [<branch>...]"));
}

#[test]
fn cache_is_in_the_help_the_command_help_and_the_man_page() {
    let (help, _, _) = run(&["help"]);
    for syntax in [
        "cache status [--json]",
        "cache prune [<entry>...] [--dry-run]",
        "cache clear <entry>...|--all [--force]",
    ] {
        assert!(collapse(&help).contains(syntax), "{syntax}");
    }
    let (one, err, exit) = run(&["help", "cache"]);
    assert_eq!((err.as_str(), exit), ("", 0));
    assert_eq!(one.matches("usage: cstan cache").count(), 3, "{one}");
    let (page, _, _) = run(&["help", "--man"]);
    assert!(man_commands(&page)
        .iter()
        .any(|l| l == "cache clear <entry>...|--all [--force]"));
}

#[test]
fn verify_is_in_the_help_and_refuses_unknown_forms() {
    let (help, _, _) = run(&["help"]);
    for syntax in [
        "verify <report-or-integration-id>",
        "verify show <run-id>",
        "verify list [<subject-id>]",
        "verify cancel <run-id>",
    ] {
        assert!(collapse(&help).contains(syntax), "{syntax}");
    }
    let (one, err, exit) = run(&["help", "verify"]);
    assert_eq!((err.as_str(), exit), ("", 0));
    assert_eq!(one.matches("usage: cstan verify").count(), 4, "{one}");
    for args in [
        &["verify"][..],
        &["verify", "frobnicate", "x"],
        &["verify", "show"],
    ] {
        let (_, err, exit) = run(args);
        assert_eq!(exit, 2, "{args:?}");
        assert!(err.contains("usage:"), "{err}");
    }
}

#[test]
fn design_guide_is_in_the_help_the_command_help_and_the_man_page() {
    let (help, _, _) = run(&["help"]);
    assert!(collapse(&help).contains("design guide [<topic>]"));
    let (design, err, exit) = run(&["help", "design"]);
    assert_eq!((err.as_str(), exit), ("", 0));
    assert!(
        design.contains("usage: cstan design guide [<topic>]"),
        "{design}"
    );
    let (man, _, _) = run(&["help", "--man"]);
    assert!(man.contains("design guide"), "{man}");
}
