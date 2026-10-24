//! `cstan design guide [<topic>]`: the built-in designer guidance, run as the real executable in an empty directory with
//! no daemon, no project and a cleared environment.
use std::path::Path;
use std::process::{Command, Output, Stdio};

const TOPICS: [(&str, &str); 7] = [
    ("main", "main.md"),
    ("rules", "rules.md"),
    ("directions-product", "directions-product.md"),
    ("directions-brand", "directions-brand.md"),
    ("redesign", "redesign.md"),
    ("proposal", "proposal.md"),
    ("record", "record.md"),
];

fn cstan(args: &[&str], cwd: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cstan"))
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .stdin(Stdio::null())
        .output()
        .expect("cstan runs")
}

fn empty_directory(name: &str) -> std::path::PathBuf {
    let dir =
        std::env::temp_dir().join(format!("cstan-design-guide-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn listing(text: &str) -> Vec<String> {
    text.lines()
        .map(|line| line.split("  ").next().unwrap().to_string())
        .collect()
}

#[test]
fn the_list_names_every_topic_in_order() {
    let dir = empty_directory("list");
    let run = cstan(&["design", "guide"], &dir);
    assert_eq!(run.status.code(), Some(0));
    assert!(run.stderr.is_empty());
    let out = String::from_utf8(run.stdout).unwrap();
    let names: Vec<&str> = TOPICS.iter().map(|(t, _)| *t).collect();
    assert_eq!(listing(&out), names);
    for line in out.lines() {
        let (_, about) = line.split_once("  ").expect("topic and description");
        assert!(!about.trim().is_empty(), "{line}");
    }
    assert_eq!(
        std::fs::read_dir(&dir).unwrap().count(),
        0,
        "writes nothing"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn each_topic_prints_its_file_exactly() {
    let dir = empty_directory("topics");
    for (topic, file) in TOPICS {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../config/src/prompts/design_guidance")
            .join(file);
        let expected = std::fs::read(&path).unwrap();
        let run = cstan(&["design", "guide", topic], &dir);
        assert_eq!(run.status.code(), Some(0), "{topic}");
        assert!(run.stderr.is_empty(), "{topic}");
        assert_eq!(run.stdout, expected, "{topic}");
    }
    assert_eq!(
        std::fs::read_dir(&dir).unwrap().count(),
        0,
        "writes nothing"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn bad_arguments_print_the_list_on_stderr_and_exit_2() {
    let dir = empty_directory("bad");
    let list = cstan(&["design", "guide"], &dir).stdout;
    for args in [
        vec!["design", "guide", "nope"],
        vec!["design", "guide", "main", "extra"],
        vec!["design", "guide", "--json"],
        vec!["design", "guide", "main", "--json"],
    ] {
        let run = cstan(&args, &dir);
        assert_eq!(run.status.code(), Some(2), "{args:?}");
        assert!(run.stdout.is_empty(), "{args:?}");
        assert_eq!(run.stderr, list, "{args:?}");
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn help_prints_the_entry() {
    let dir = empty_directory("help");
    for flag in ["--help", "-h"] {
        let run = cstan(&["design", "guide", flag], &dir);
        assert_eq!(run.status.code(), Some(0));
        assert!(run.stderr.is_empty());
        let out = String::from_utf8(run.stdout).unwrap();
        assert!(
            out.starts_with("usage: cstan design guide [<topic>]\n"),
            "{out}"
        );
        assert!(!out.contains("design show"), "{out}");
    }
    for args in [vec!["help", "design"], vec!["design", "--help"]] {
        let run = cstan(&args, &dir);
        assert_eq!(run.status.code(), Some(0), "{args:?}");
        let out = String::from_utf8(run.stdout).unwrap();
        assert!(
            out.contains("usage: cstan design guide [<topic>]"),
            "{args:?}: {out}"
        );
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn other_design_commands_stay_routed() {
    let dir = empty_directory("routed");
    let run = cstan(&["design", "show", "x"], &dir);
    assert_ne!(run.status.code(), Some(0));
    assert!(run.stdout.is_empty());
    let err = String::from_utf8(run.stderr).unwrap();
    assert!(!err.contains("directions-product"), "{err}");
    assert!(!err.is_empty());
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    std::fs::remove_dir_all(&dir).unwrap();
}
