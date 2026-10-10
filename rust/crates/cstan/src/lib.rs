//! The cstan CLI: every command of the Node CLI (`src/cli.ts`), byte for byte, as a library the `cstan` binary and the
//! transcript replay both call. A run takes a `Context` (arguments, environment, working directory, clock) and ends in an
//! `Outcome`: the bytes and exit code to give, or one of the few things only the process can do (replace itself with
//! `cstan-dash`, serve the daemon, poll `status --watch`).
pub mod agent;
pub mod config_cmd;
pub mod dash;
pub mod git;
pub mod init;
pub mod jsops;
pub mod operator;
pub mod render;
pub mod routed;
pub mod status;
pub mod usage;
pub mod watch;

use std::ffi::OsString;
use std::path::PathBuf;

use capstan_wire::js::{self, JsStr, Value};

use jsops::{JsError, EXIT_BLOCKED, EXIT_INVALID, EXIT_RUNTIME};

pub use usage::USAGE;

/// What `cstan __front-version` prints, for the installer.
pub const VERSION: &str = env!("CSTAN_FRONT_VERSION");

pub const EXIT_USAGE: i32 = 2;

/// Everything a run depends on, so a test can replay it without touching the process.
pub struct Context {
    pub args: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>,
    pub cwd: PathBuf,
    /// `Date.now()`.
    pub now_ms: f64,
    /// The `cstan` executable: the daemon is started as `<cli_path> daemon`.
    pub cli_path: PathBuf,
    /// Whether standard input and output are a terminal; `None` asks the process. A test sets it.
    pub terminal: Option<bool>,
}

/// A program to replace this process with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecPlan {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>,
}

/// `cstan status --watch`: poll the daemon at `socket` every `interval_seconds`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchPlan {
    pub socket: PathBuf,
    pub credential: String,
    pub interval_seconds: u64,
}

/// What only the process itself can do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Serve {
    /// `cstan daemon`.
    Daemon,
    /// `cstan __restart-helper <plan>`.
    RestartHelper(PathBuf),
}

/// The end of a run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Done {
        stdout: Vec<u8>,
        stderr: Vec<u8>,
        exit: i32,
    },
    /// Replace the process (after writing `stderr`).
    Exec {
        plan: ExecPlan,
        stderr: Vec<u8>,
    },
    /// Poll the daemon; `stderr` holds what was already said.
    Watch {
        plan: WatchPlan,
        stderr: Vec<u8>,
    },
    Serve(Serve),
}

/// Standard output and error of a run, in the order the Node CLI writes each.
#[derive(Default)]
pub struct Io {
    pub out: Vec<u8>,
    pub err: Vec<u8>,
}

impl Io {
    pub fn out_line(&mut self, text: &str) {
        self.out.extend_from_slice(text.as_bytes());
        self.out.push(b'\n');
    }

    pub fn out_js_line(&mut self, text: &JsStr) {
        self.out.extend_from_slice(text.to_utf8_lossy().as_bytes());
        self.out.push(b'\n');
    }

    pub fn err_line(&mut self, text: &str) {
        self.err.extend_from_slice(text.as_bytes());
        self.err.push(b'\n');
    }
}

/// Why a command ends in failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fail {
    /// `usage()`: the usage text, then the catch-all's `cstan: <usage>`, exit 2.
    Usage,
    /// An error the catch-all words as `cstan: <message>`.
    Error(JsError),
}

impl From<JsError> for Fail {
    fn from(error: JsError) -> Fail {
        Fail::Error(error)
    }
}

pub fn invalid(message: impl Into<JsStr>) -> Fail {
    Fail::Error(JsError {
        exit: EXIT_INVALID,
        message: message.into(),
    })
}

pub fn blocked(message: impl Into<JsStr>) -> Fail {
    Fail::Error(JsError {
        exit: EXIT_BLOCKED,
        message: message.into(),
    })
}

pub fn runtime(message: impl Into<JsStr>) -> Fail {
    Fail::Error(JsError {
        exit: EXIT_RUNTIME,
        message: message.into(),
    })
}

/// What a command that did not fail ends with.
pub enum Flow {
    Code(i32),
    Exec(ExecPlan),
    Watch(WatchPlan),
    Serve(Serve),
}

/// One run: the context, the output so far and the once-only foreign-socket warning.
pub struct Cli<'a> {
    pub ctx: &'a Context,
    pub io: Io,
    pub foreign_warned: bool,
}

/// Runs one invocation.
pub fn run(context: &Context) -> Outcome {
    let mut cli = Cli {
        ctx: context,
        io: Io::default(),
        foreign_warned: false,
    };
    let result = cli.run_cli();
    let Io { out, mut err } = cli.io;
    match result {
        Ok(Flow::Code(exit)) => Outcome::Done {
            stdout: out,
            stderr: err,
            exit,
        },
        Ok(Flow::Exec(plan)) => Outcome::Exec { plan, stderr: err },
        Ok(Flow::Watch(plan)) => Outcome::Watch { plan, stderr: err },
        Ok(Flow::Serve(serve)) => Outcome::Serve(serve),
        Err(Fail::Usage) => {
            err.extend_from_slice(format!("{USAGE}\ncstan: {USAGE}\n").as_bytes());
            Outcome::Done {
                stdout: out,
                stderr: err,
                exit: EXIT_USAGE,
            }
        }
        Err(Fail::Error(error)) => {
            err.extend_from_slice(
                jsops::concat(&[&"cstan: ".into(), &error.message, &"\n".into()])
                    .to_utf8_lossy()
                    .as_bytes(),
            );
            Outcome::Done {
                stdout: out,
                stderr: err,
                exit: error.exit,
            }
        }
    }
}

/// `parseOptions`: `--json` before a `--` is a flag, everything after it is literal.
pub fn parse_options(args: &[String]) -> (Vec<String>, bool) {
    let (options, literal) = match args.iter().position(|a| a == "--") {
        Some(at) => (&args[..at], &args[at + 1..]),
        None => (args, &args[..0]),
    };
    let positional = options
        .iter()
        .filter(|a| *a != "--json")
        .chain(literal)
        .cloned()
        .collect();
    (positional, options.iter().any(|a| a == "--json"))
}

/// `takeFlag`: removes `name` from `flags`; true when it was there.
pub fn take_flag(flags: &mut Vec<String>, name: &str) -> bool {
    match flags.iter().position(|f| f == name) {
        Some(at) => {
            flags.remove(at);
            true
        }
        None => false,
    }
}

/// `takeIntervalSeconds`: removes `--interval N` and returns N; 2 when absent.
pub fn take_interval_seconds(flags: &mut Vec<String>) -> Result<u64, Fail> {
    let Some(at) = flags.iter().position(|f| f == "--interval") else {
        return Ok(2);
    };
    let value = flags.get(at + 1);
    let seconds = value.and_then(|v| {
        let bytes = v.as_bytes();
        let shape = matches!(bytes.len(), 1 | 2)
            && bytes.iter().all(u8::is_ascii_digit)
            && bytes[0] != b'0';
        shape.then(|| v.parse::<u64>().ok()).flatten()
    });
    match seconds {
        Some(seconds) if seconds <= 60 => {
            flags.drain(at..at + 2);
            Ok(seconds)
        }
        _ => Err(invalid("--interval must be an integer from 1 to 60")),
    }
}

/// A serde_json value as the JavaScript value `JSON.parse` of its text gives.
pub fn to_js(value: &serde_json::Value) -> Value {
    js::parse(&value.to_string()).expect("serde_json writes JSON")
}

/// `output(value, json)`; `None` is `undefined`.
pub fn output(out: &mut Vec<u8>, value: Option<&Value>, json: bool) {
    let rendered = match (value, json) {
        (None, _) => JsStr::from("undefined"),
        (Some(v), true) => js::stringify(v, 2),
        (Some(v), false) => js::render(v),
    };
    out.extend_from_slice(rendered.to_utf8_lossy().as_bytes());
    out.push(b'\n');
}

impl Cli<'_> {
    fn run_cli(&mut self) -> Result<Flow, Fail> {
        let args: Vec<String> = self
            .ctx
            .args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let Some((command, rest)) = args.split_first() else {
            return Err(Fail::Usage);
        };
        let rest = rest.to_vec();
        match command.as_str() {
            "--version" | "-V" | "version" => {
                if !rest.is_empty() {
                    return Err(Fail::Usage);
                }
                self.io.out_line(&format!("cstan {VERSION}"));
                return Ok(Flow::Code(0));
            }
            "__front-version" if rest.is_empty() => {
                self.io.out_line(&format!("cstan-front {VERSION}"));
                return Ok(Flow::Code(0));
            }
            "--help" | "-h" | "help" => {
                self.io.out_line(USAGE);
                return Ok(Flow::Code(0));
            }
            "__restart-helper" => return self.restart_helper(&rest),
            "init" => return self.init(&rest),
            "config" => return self.config(&rest),
            "herdr-config" => {
                if !rest.is_empty() {
                    return Err(Fail::Usage);
                }
                self.io
                    .out
                    .extend_from_slice(usage::HERDR_CONFIG_SNIPPET.as_bytes());
                return Ok(Flow::Code(0));
            }
            "daemon" => {
                if !rest.is_empty() {
                    return Err(Fail::Usage);
                }
                return Ok(Flow::Serve(Serve::Daemon));
            }
            "start" => return self.start(&rest),
            "stop" => return self.stop(&rest),
            "inbox" if rest.len() == 1 && rest[0] == "--hook" => return self.inbox_hook(),
            _ => {}
        }
        if command == "ping" || routed::is_routed(command) || (command == "pm" && pm_restart(&rest))
        {
            let (mut positional, json) = parse_options(&rest);
            if command == "pm" {
                positional.remove(0);
            }
            let name = if command == "pm" {
                "pm-restart"
            } else {
                command.as_str()
            };
            if name == "ping" && !positional.is_empty() {
                return Err(Fail::Usage);
            }
            return self.run_routed(name, &positional, json);
        }
        if command == "cancel" {
            let (positional, json) = parse_options(&rest);
            if positional.len() == 1 {
                return self.run_routed("cancel", &positional, json);
            }
        }
        let before_separator = match rest.iter().position(|a| a == "--") {
            Some(at) => &rest[..at],
            None => &rest[..],
        };
        if command == "status" && before_separator.iter().any(|a| a == "--watch") {
            return self.status_watch(&rest);
        }
        if command == "dash" {
            return self.dash(&rest);
        }
        if command == "status" && self.agent_environment()?.is_some() {
            let (positional, json) = parse_options(&rest);
            if !positional.is_empty() {
                return Err(Fail::Usage);
            }
            return self.run_routed("status", &[], json);
        }
        if command == "status" {
            return self.status_offline(&rest);
        }
        if command == "inspect" {
            return self.inspect(&rest);
        }
        Err(Fail::Usage)
    }

    fn restart_helper(&mut self, rest: &[String]) -> Result<Flow, Fail> {
        match rest.first() {
            None => {
                self.io.err_line("usage: helper.mjs <plan.json>");
                Ok(Flow::Code(64))
            }
            Some(_) => {
                let plan = self.ctx.args.get(1).map(PathBuf::from).unwrap_or_default();
                Ok(Flow::Serve(Serve::RestartHelper(plan)))
            }
        }
    }
}

/// `command === "pm" && rest.filter((arg) => arg !== "--json")[0] === "restart"`.
fn pm_restart(rest: &[String]) -> bool {
    rest.iter()
        .find(|a| *a != "--json")
        .is_some_and(|a| a == "restart")
}
