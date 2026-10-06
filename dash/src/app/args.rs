//! The `cstan-dash` command line: flags in any order, the credential from the environment.
use std::path::PathBuf;

pub const USAGE: &str = "usage: cstan-dash --socket <path> [--worker-limit <n>] [--interval <seconds>] [--no-color] [--reduced-motion]";
pub const CREDENTIAL_VARIABLE: &str = "CSTAN_DASH_CREDENTIAL";
pub const NO_CREDENTIAL_MESSAGE: &str = "no operator credential; start it with cstan dash";
pub const DEFAULT_INTERVAL_SECONDS: u32 = 2;

pub const EXIT_RUNTIME: u8 = 1;
pub const EXIT_USAGE: u8 = 2;
pub const EXIT_INTERVAL: u8 = 3;

/// What the flags ask for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    pub socket: PathBuf,
    pub worker_limit: Option<i64>,
    pub interval_seconds: u32,
    pub no_color: bool,
    pub reduced_motion: bool,
}

/// An invalid command line: what to print on stderr (the message line, then the usage line when `show_usage`) and the exit code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArgError {
    pub code: u8,
    pub message: String,
    pub show_usage: bool,
}

impl ArgError {
    fn usage(message: impl Into<String>) -> Self {
        ArgError {
            code: EXIT_USAGE,
            message: message.into(),
            show_usage: true,
        }
    }

    /// The text for stderr, one line or two.
    pub fn render(&self) -> String {
        if self.show_usage {
            format!("cstan-dash: {}\n{USAGE}\n", self.message)
        } else {
            format!("cstan-dash: {}\n", self.message)
        }
    }
}

fn value_of<'a>(flag: &str, args: &'a [String], at: &mut usize) -> Result<&'a str, ArgError> {
    *at += 1;
    args.get(*at)
        .map(String::as_str)
        .ok_or_else(|| ArgError::usage(format!("{flag} needs a value")))
}

/// `1` to `60`, no sign, no leading zero.
fn parse_interval(text: &str) -> Option<u32> {
    let bytes = text.as_bytes();
    let plain =
        matches!(bytes.len(), 1 | 2) && bytes[0] != b'0' && bytes.iter().all(u8::is_ascii_digit);
    let seconds: u32 = plain.then(|| text.parse().ok()).flatten()?;
    (1..=60).contains(&seconds).then_some(seconds)
}

pub fn parse_args(args: &[String]) -> Result<Options, ArgError> {
    let mut socket: Option<PathBuf> = None;
    let mut worker_limit = None;
    let mut interval_seconds = DEFAULT_INTERVAL_SECONDS;
    let mut no_color = false;
    let mut reduced_motion = false;
    let mut at = 0;
    while at < args.len() {
        let flag = args[at].as_str();
        match flag {
            "--socket" => {
                let path = PathBuf::from(value_of(flag, args, &mut at)?);
                if !path.is_absolute() {
                    return Err(ArgError::usage("--socket must be an absolute path"));
                }
                socket = Some(path);
            }
            "--worker-limit" => {
                let text = value_of(flag, args, &mut at)?;
                let limit = text
                    .parse::<i64>()
                    .ok()
                    .filter(|n| *n >= 0 && text.bytes().all(|b| b.is_ascii_digit()));
                worker_limit = Some(limit.ok_or_else(|| {
                    ArgError::usage("--worker-limit must be a non-negative integer")
                })?);
            }
            "--interval" => {
                let text = value_of(flag, args, &mut at)?;
                interval_seconds = parse_interval(text).ok_or_else(|| ArgError {
                    code: EXIT_INTERVAL,
                    message: "--interval must be an integer from 1 to 60".to_string(),
                    show_usage: false,
                })?;
            }
            "--no-color" => no_color = true,
            "--reduced-motion" => reduced_motion = true,
            other => return Err(ArgError::usage(format!("unknown argument {other}"))),
        }
        at += 1;
    }
    let socket = socket.ok_or_else(|| ArgError::usage("--socket is required"))?;
    Ok(Options {
        socket,
        worker_limit,
        interval_seconds,
        no_color,
        reduced_motion,
    })
}

/// The credential, or the exit-2 error when it is missing or empty.
pub fn credential_from(value: Option<String>) -> Result<String, ArgError> {
    match value {
        Some(text) if !text.is_empty() => Ok(text),
        _ => Err(ArgError {
            code: EXIT_USAGE,
            message: NO_CREDENTIAL_MESSAGE.to_string(),
            show_usage: false,
        }),
    }
}
