//! The real terminal: raw mode, the alternate screen and the rules for colour and glyphs; restored on every way out.
use std::io::{self, Write};

use crossterm::cursor::{Hide, Show};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};

pub const NOT_A_TERMINAL_MESSAGE: &str = "dash needs an interactive terminal; use \"cstan status\" (or \"cstan status --watch\") for scripts and pipes";

/// A variable of the environment, by name.
pub trait Env {
    fn get(&self, name: &str) -> Option<String>;
}

impl<F: Fn(&str) -> Option<String>> Env for F {
    fn get(&self, name: &str) -> Option<String> {
        self(name)
    }
}

/// Whether the locale names a UTF-8 charset: the first of LC_ALL, LC_CTYPE and LANG that is set decides; with none set
/// the terminal is assumed to be UTF-8.
pub fn is_utf8_locale(env: &dyn Env) -> bool {
    let value = ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .filter_map(|name| env.get(name))
        .find(|v| !v.is_empty());
    match value {
        None => true,
        Some(text) => {
            let lower = text.to_ascii_lowercase();
            lower.contains("utf8") || lower.contains("utf-8")
        }
    }
}

/// ASCII glyphs are used on a dumb terminal and under a non-UTF-8 locale.
pub fn wants_ascii(env: &dyn Env) -> bool {
    env.get("TERM").as_deref() == Some("dumb") || !is_utf8_locale(env)
}

/// Colour is off for `--no-color`, a non-empty `NO_COLOR` and `TERM=dumb`.
pub fn wants_no_color(flag: bool, env: &dyn Env) -> bool {
    flag || env.get("NO_COLOR").is_some_and(|v| !v.is_empty())
        || env.get("TERM").as_deref() == Some("dumb")
}

/// Reduced motion is the flag or `CSTAN_REDUCED_MOTION=1`.
pub fn wants_reduced_motion(flag: bool, env: &dyn Env) -> bool {
    flag || env.get("CSTAN_REDUCED_MOTION").as_deref() == Some("1")
}

/// Entering and leaving the full-screen mode.
pub trait TerminalOps {
    fn enter(&mut self) -> io::Result<()>;
    fn leave(&mut self);
}

/// Raw mode, the alternate screen and a hidden cursor on the real terminal.
pub struct CrosstermOps;

impl TerminalOps for CrosstermOps {
    fn enter(&mut self) -> io::Result<()> {
        enable_raw_mode()?;
        if let Err(error) = execute!(io::stdout(), EnterAlternateScreen, Hide) {
            let _ = disable_raw_mode();
            return Err(error);
        }
        Ok(())
    }

    fn leave(&mut self) {
        restore_terminal();
    }
}

/// Puts the terminal back as it was; safe to call more than once and from any thread.
pub fn restore_terminal() {
    let mut out = io::stdout();
    let _ = execute!(out, Show, LeaveAlternateScreen);
    let _ = out.flush();
    let _ = disable_raw_mode();
}

/// Holds the full-screen mode: `leave` runs exactly once when this is dropped, also while a panic unwinds.
pub struct Session<T: TerminalOps> {
    ops: T,
    entered: bool,
}

impl<T: TerminalOps> Session<T> {
    pub fn enter(mut ops: T) -> io::Result<Self> {
        ops.enter()?;
        Ok(Session { ops, entered: true })
    }

    /// Leaves now rather than at drop.
    pub fn leave(&mut self) {
        if self.entered {
            self.entered = false;
            self.ops.leave();
        }
    }
}

impl<T: TerminalOps> Drop for Session<T> {
    fn drop(&mut self) {
        self.leave();
    }
}

/// Makes a panic restore the terminal before its message prints, so the message stays readable.
pub fn install_panic_hook(restore: fn()) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        previous(info);
    }));
}

/// A signal that asks the dashboard to end.
pub fn quit_signals() -> [i32; 3] {
    [
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGHUP,
        signal_hook::consts::SIGINT,
    ]
}
