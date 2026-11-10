//! `cstan plan archive`: the question and its answers over any streams, the refusal when nobody can answer, and the
//! client's flow (one preview call, the list, the question, the second call with the token) with a fake terminal.
mod support;

use std::cell::RefCell;
use std::io::{Cursor, Write};
use std::process::{Command, Stdio};
use std::rc::Rc;

use capstan_wire::js::{self, Value};
use cstan_front::plan_confirm::{self, Confirm, PlanCall, StreamConfirm};
use cstan_front::{Fail, Io};
use support::FRONT;

/// One buffer both the list and the question are written to, so their order can be read back.
#[derive(Clone, Default)]
struct Shared(Rc<RefCell<Vec<u8>>>);

impl Write for Shared {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.borrow_mut().extend_from_slice(b"<flush>");
        Ok(())
    }
}

fn answer(text: &str) -> (bool, String) {
    let screen = Shared::default();
    let mut confirm = StreamConfirm::new(
        Cursor::new(text.as_bytes().to_vec()),
        screen.clone(),
        screen.clone(),
        true,
    );
    let yes = confirm
        .ask("plan-1 draft One\n", "Archive 1 plan(s)? [y/N] ")
        .unwrap();
    let shown = String::from_utf8(screen.0.borrow().clone()).unwrap();
    (yes, shown)
}

#[test]
fn the_list_is_flushed_before_the_question_and_the_question_before_the_read() {
    let (_, shown) = answer("y\n");
    assert_eq!(
        shown,
        "plan-1 draft One\n<flush>Archive 1 plan(s)? [y/N] <flush>"
    );
}

#[test]
fn y_and_yes_in_any_case_agree_and_everything_else_does_not() {
    for yes in ["y\n", "yes\n", "Y\n", "YES\n", "Yes\n", " y \n", "y"] {
        assert!(answer(yes).0, "{yes:?}");
    }
    for no in ["n\n", "N\n", "\n", "", "no\n", "yep\n", "ye\n", "y es\n"] {
        assert!(!answer(no).0, "{no:?}");
    }
}

#[test]
fn a_stream_confirm_over_nothing_is_not_a_terminal() {
    let confirm = StreamConfirm::new(Cursor::new(Vec::new()), Vec::new(), Vec::new(), false);
    assert!(!confirm.is_terminal());
}

#[test]
fn the_binary_with_piped_input_and_no_yes_refuses_before_any_call() {
    let scratch = support::Scratch::new("plan-archive", 0);
    for args in [
        vec!["plan", "archive", "plan-1"],
        vec!["plan", "archive", "--stale"],
        vec!["plan", "--stale", "--archive"],
        vec!["plan", "show", "--draft", "--archive"],
        vec!["plan", "archive", "plan-1", "--json"],
    ] {
        let mut child = Command::new(FRONT)
            .args(&args)
            .current_dir(&scratch.0)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("cstan runs");
        drop(child.stdin.take());
        let out = child.wait_with_output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert_eq!(
            String::from_utf8_lossy(&out.stderr),
            "cstan: plan archive needs a terminal to confirm, or --yes\n",
            "{args:?}"
        );
        assert!(out.stdout.is_empty(), "{args:?}");
    }
    // Nothing was started or written in the folder.
    assert_eq!(std::fs::read_dir(&scratch.0).unwrap().count(), 0);
}

/// A scripted controller: it answers the preview, then the confirm, and keeps what it was asked.
struct Fake {
    calls: Vec<Vec<String>>,
    preview: Value,
    done: Value,
}

impl Fake {
    fn new(preview: &str, done: &str) -> Fake {
        Fake {
            calls: Vec::new(),
            preview: js::parse(preview).unwrap(),
            done: js::parse(done).unwrap(),
        }
    }
}

impl PlanCall for Fake {
    fn call(&mut self, args: &[String]) -> Result<Option<Value>, Fail> {
        self.calls.push(args.to_vec());
        Ok(Some(if args.iter().any(|a| a == "--confirm") {
            self.done.clone()
        } else {
            self.preview.clone()
        }))
    }
}

/// A terminal that answers as scripted and records what it was shown.
struct Terminal {
    terminal: bool,
    reply: bool,
    asked: Vec<(String, String)>,
}

impl Confirm for Terminal {
    fn is_terminal(&self) -> bool {
        self.terminal
    }
    fn ask(&mut self, list: &str, question: &str) -> std::io::Result<bool> {
        self.asked.push((list.to_string(), question.to_string()));
        Ok(self.reply)
    }
}

const PREVIEW: &str = r#"{"preview":true,"token":"0123456789abcdef","plans":[{"planId":"plan-1","status":"draft","title":"One"},{"planId":"plan-2","status":"approved","title":"Two","stale":["idle","done_unsigned"]}]}"#;
const DONE: &str = r#"{"archived":["plan-1","plan-2"]}"#;

fn words(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_string).collect()
}

fn run(
    args: &str,
    json: bool,
    terminal: bool,
    reply: bool,
    preview: &str,
) -> (Result<i32, Fail>, Fake, Terminal, Io) {
    let mut fake = Fake::new(preview, DONE);
    let mut tty = Terminal {
        terminal,
        reply,
        asked: Vec::new(),
    };
    let mut io = Io::default();
    let result = plan_confirm::run(&words(args), json, &mut tty, &mut fake, &mut io);
    (result, fake, tty, io)
}

fn out(io: &Io) -> String {
    String::from_utf8_lossy(&io.out).into_owned()
}

#[test]
fn on_a_terminal_y_archives_with_the_token_of_the_list_shown() {
    let (result, fake, tty, io) = run("archive --stale", false, true, true, PREVIEW);
    assert_eq!(result.unwrap(), 0);
    assert_eq!(
        tty.asked,
        [(
            "plan-1 draft One\nplan-2 approved Two stale: idle, done_unsigned\n".to_string(),
            "Archive 2 plan(s)? [y/N] ".to_string()
        )]
    );
    assert_eq!(fake.calls.len(), 2);
    assert_eq!(fake.calls[0], words("archive --stale"));
    assert_eq!(
        fake.calls[1],
        words("archive --stale --confirm 0123456789abcdef")
    );
    assert_eq!(out(&io), "archived 2: plan-1, plan-2\n");
}

#[test]
fn on_a_terminal_n_archives_nothing_and_exits_1() {
    let (result, fake, tty, io) = run("archive plan-1", false, true, false, PREVIEW);
    assert_eq!(result.unwrap(), 1);
    assert_eq!(tty.asked.len(), 1);
    assert_eq!(fake.calls.len(), 1, "no second call");
    assert_eq!(out(&io), "nothing archived\n");
}

#[test]
fn yes_skips_the_question_not_the_list_and_is_not_sent() {
    let (result, fake, tty, io) = run("archive --stale --yes", false, false, false, PREVIEW);
    assert_eq!(result.unwrap(), 0);
    assert!(tty.asked.is_empty());
    assert_eq!(fake.calls[0], words("archive --stale"));
    assert_eq!(
        fake.calls[1],
        words("archive --stale --confirm 0123456789abcdef")
    );
    assert_eq!(
        out(&io),
        "plan-1 draft One\nplan-2 approved Two stale: idle, done_unsigned\narchived 2: plan-1, plan-2\n"
    );
}

#[test]
fn without_a_terminal_and_without_yes_nothing_is_called() {
    let (result, fake, tty, io) = run("archive --stale", false, false, true, PREVIEW);
    match result {
        Err(Fail::Error(error)) => {
            assert_eq!(error.exit, 2);
            assert_eq!(
                error.message.to_utf8_lossy(),
                "plan archive needs a terminal to confirm, or --yes"
            );
        }
        other => panic!("expected the refusal, got {:?}", other.map(|_| ())),
    }
    assert!(fake.calls.is_empty());
    assert!(tty.asked.is_empty());
    assert!(io.out.is_empty());
    // Also with --json.
    let (result, fake, _, _) = run("archive --stale", true, false, true, PREVIEW);
    assert!(matches!(result, Err(Fail::Error(error)) if error.exit == 2));
    assert!(fake.calls.is_empty());
}

#[test]
fn an_empty_preview_asks_nothing_and_calls_once() {
    let empty = r#"{"preview":true,"token":"e3b0c44298fc1c14","plans":[]}"#;
    for (terminal, yes) in [(true, false), (false, true)] {
        let args = if yes {
            "archive --stale --yes"
        } else {
            "archive --stale"
        };
        let (result, fake, tty, io) = run(args, false, terminal, true, empty);
        assert_eq!(result.unwrap(), 0);
        assert!(tty.asked.is_empty());
        assert_eq!(fake.calls.len(), 1);
        assert_eq!(out(&io), "no plans match\n");
    }
}

#[test]
fn json_keeps_standard_output_one_document() {
    let (result, _, tty, io) = run("archive --stale", true, true, true, PREVIEW);
    assert_eq!(result.unwrap(), 0);
    // The list goes with the question, on standard error's side.
    assert_eq!(tty.asked[0].0, "");
    assert!(tty.asked[0].1.starts_with("plan-1 draft One\n"));
    assert!(tty.asked[0].1.ends_with("Archive 2 plan(s)? [y/N] "));
    assert!(js::parse(&out(&io)).unwrap().get("archived").is_some());
}

#[test]
fn is_archive_is_for_archive_and_the_archive_flag_without_a_token() {
    for yes in [
        "archive plan-1",
        "archive --stale",
        "--stale --archive",
        "show --draft --archive",
    ] {
        assert!(plan_confirm::is_archive(&words(yes)), "{yes}");
    }
    for no in [
        "show",
        "--stale",
        "unarchive plan-1",
        "open normal T",
        "archive plan-1 --confirm abc",
    ] {
        assert!(!plan_confirm::is_archive(&words(no)), "{no}");
    }
}
