//! `cstan plan archive` and `cstan plan show ... --archive`: the client asks the controller for a preview of the plans an
//! archive would hide, shows the list, asks the operator and only then sends the token of that list back to archive them.
use std::io::{BufRead, IsTerminal, Write};

use capstan_wire::js::{JsStr, Value};

use crate::jsops::JsError;
use crate::{output, Fail, Io};

/// The refusal made before any call when nobody can answer the question.
pub const NEEDS_TERMINAL: &str = "plan archive needs a terminal to confirm, or --yes";

/// Where the question is asked.
pub trait Confirm {
    /// Whether there is somebody to ask.
    fn is_terminal(&self) -> bool;
    /// Shows `list` (standard output) and then asks `question` (standard error); true for `y` or `yes` in any case.
    fn ask(&mut self, list: &str, question: &str) -> std::io::Result<bool>;
}

/// A `Confirm` over any streams: the answers come from `input`, the list goes to `out` and the question to `err`.
pub struct StreamConfirm<R: BufRead, O: Write, E: Write> {
    input: R,
    out: O,
    err: E,
    terminal: bool,
}

impl<R: BufRead, O: Write, E: Write> StreamConfirm<R, O, E> {
    pub fn new(input: R, out: O, err: E, terminal: bool) -> Self {
        Self {
            input,
            out,
            err,
            terminal,
        }
    }

    pub fn into_parts(self) -> (R, O, E) {
        (self.input, self.out, self.err)
    }
}

impl<R: BufRead, O: Write, E: Write> Confirm for StreamConfirm<R, O, E> {
    fn is_terminal(&self) -> bool {
        self.terminal
    }

    fn ask(&mut self, list: &str, question: &str) -> std::io::Result<bool> {
        // The list is on the screen before the question is, and the question before the read.
        self.out.write_all(list.as_bytes())?;
        self.out.flush()?;
        self.err.write_all(question.as_bytes())?;
        self.err.flush()?;
        let mut answer = String::new();
        self.input.read_line(&mut answer)?;
        let answer = answer.trim().to_ascii_lowercase();
        Ok(answer == "y" || answer == "yes")
    }
}

/// The real streams of the process: standard input, output and error; a terminal when standard input is one.
pub fn stdio() -> StreamConfirm<std::io::BufReader<std::io::Stdin>, std::io::Stdout, std::io::Stderr>
{
    StreamConfirm::new(
        std::io::BufReader::new(std::io::stdin()),
        std::io::stdout(),
        std::io::stderr(),
        std::io::stdin().is_terminal(),
    )
}

/// One call to the controller: the `plan` command with these arguments; the result of an accepted call.
pub trait PlanCall {
    fn call(&mut self, args: &[String]) -> Result<Option<Value>, Fail>;
}

/// Whether the arguments of `plan` ask for an archive that this module confirms: `archive` or `--archive`, without a
/// token already given.
pub fn is_archive(args: &[String]) -> bool {
    let archive =
        args.first().is_some_and(|a| a == "archive") || args.iter().any(|a| a == "--archive");
    archive && !args.iter().any(|a| a == "--confirm")
}

fn text(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "-".to_string(),
        Some(v) => crate::jsops::to_js_string(v).to_utf8_lossy(),
    }
}

/// One line per plan of the preview: id, status, title and, for a stale plan, its reasons.
fn preview_lines(plans: &[Value]) -> Vec<String> {
    plans
        .iter()
        .map(|p| {
            let mut line = format!(
                "{} {} {}",
                text(p.get("planId")),
                text(p.get("status")),
                text(p.get("title"))
            );
            if let Some(Value::Array(reasons)) = p.get("stale") {
                let reasons: Vec<String> = reasons.iter().map(|r| text(Some(r))).collect();
                line.push_str(&format!(" stale: {}", reasons.join(", ")));
            }
            line
        })
        .collect()
}

/// `plan archive`: preview, list, question, archive. Returns the exit code.
pub fn run(
    args: &[String],
    json: bool,
    confirm: &mut dyn Confirm,
    caller: &mut dyn PlanCall,
    io: &mut Io,
) -> Result<i32, Fail> {
    let yes = args.iter().any(|a| a == "--yes");
    let args: Vec<String> = args.iter().filter(|a| *a != "--yes").cloned().collect();
    if !yes && !confirm.is_terminal() {
        return Err(Fail::Error(JsError {
            exit: crate::EXIT_USAGE,
            message: JsStr::from(NEEDS_TERMINAL),
        }));
    }
    let preview = caller.call(&args)?;
    let preview = preview.as_ref();
    let is_preview = matches!(
        preview.and_then(|p| p.get("preview")),
        Some(Value::Bool(true))
    );
    let (Some(preview), true) = (preview, is_preview) else {
        // A controller that did not answer with a preview: show what it said.
        output(&mut io.out, preview, json);
        return Ok(0);
    };
    let plans: Vec<Value> = match preview.get("plans") {
        Some(Value::Array(plans)) => plans.clone(),
        _ => Vec::new(),
    };
    if plans.is_empty() {
        if json {
            output(&mut io.out, Some(preview), true);
        } else {
            io.out_line("no plans match");
        }
        return Ok(0);
    }
    let mut list = preview_lines(&plans).join("\n");
    list.push('\n');
    let question = format!("Archive {} plan(s)? [y/N] ", plans.len());
    if yes {
        // The list is still shown; with --json it goes to standard error so standard output stays one document.
        if json {
            io.err.extend_from_slice(list.as_bytes());
        } else {
            io.out.extend_from_slice(list.as_bytes());
        }
    } else {
        let answered = if json {
            confirm.ask("", &format!("{list}{question}"))
        } else {
            confirm.ask(&list, &question)
        };
        let agreed = answered.map_err(|e| crate::runtime(e.to_string()))?;
        if !agreed {
            io.out_line("nothing archived");
            return Ok(1);
        }
    }
    let mut again = args.clone();
    again.push("--confirm".to_string());
    again.push(text(preview.get("token")));
    let done = caller.call(&again)?;
    if json {
        output(&mut io.out, done.as_ref(), true);
    } else if let Some(Value::Array(ids)) = done.as_ref().and_then(|d| d.get("archived")) {
        let ids: Vec<String> = ids.iter().map(|i| text(Some(i))).collect();
        if ids.is_empty() {
            io.out_line("no plans match");
        } else {
            io.out_line(&format!("archived {}: {}", ids.len(), ids.join(", ")));
        }
    } else {
        output(&mut io.out, done.as_ref(), false);
    }
    Ok(0)
}
