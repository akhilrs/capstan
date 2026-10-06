//! The terminal loop: input, polling and timers on one thread's receiver; the screen is redrawn only when what it shows changes.
pub mod args;
pub mod client;
pub mod keys;
pub mod paint;
pub mod poller;
pub mod state;
pub mod terminal;

use std::io::{self, IsTerminal, Write};
use std::process::ExitCode;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crossterm::event::{self, Event as TerminalEvent};
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::layout::Rect;
use ratatui::Terminal;

use crate::model::DashAction;
use crate::view::{make_theme, Overlay, Size, Theme, ThemeOptions, ViewState};
use client::{CallResult, Client};
use paint::ColorDepth;
use poller::{PollEvent, Poller, BACKOFF_MS};
use state::{AppState, Effect, Hooks, OverlayKind, SPINNER_MS};

/// Everything the loop waits for.
#[allow(clippy::large_enum_variant)]
pub enum Event {
    Input(TerminalEvent),
    Poll(PollEvent),
    CallDone {
        action: DashAction,
        result: CallResult,
    },
    /// A termination signal (its number).
    Signal(i32),
}

/// What the loop asks of the outside world.
pub trait Controls {
    fn poll_now(&self);
    fn set_paused(&self, paused: bool);
    fn set_interval_ms(&self, interval_ms: u64);
    /// Runs the action's daemon call without blocking; the answer comes back as `Event::CallDone`.
    fn call(&self, action: DashAction);
}

/// Controls that do nothing.
pub struct NoControls;

impl Controls for NoControls {
    fn poll_now(&self) {}
    fn set_paused(&self, _paused: bool) {}
    fn set_interval_ms(&self, _interval_ms: u64) {}
    fn call(&self, _action: DashAction) {}
}

/// What decides whether the screen needs painting again.
#[derive(Clone, Debug, PartialEq)]
struct DrawKey {
    view: ViewState,
    overlay: Option<OverlayKind>,
    model_rev: u64,
    placeholder: Option<String>,
}

pub struct Runtime<B: Backend> {
    pub state: AppState,
    pub terminal: Terminal<B>,
    theme: Theme,
    hooks: Hooks,
    depth: ColorDepth,
    controls: Box<dyn Controls>,
    bell: Box<dyn FnMut() + Send>,
    drawn: Option<DrawKey>,
    /// How many times the screen was painted.
    pub draws: usize,
    spinner_at: Option<i64>,
    pub exit: Option<u8>,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

impl<B: Backend> Runtime<B> {
    pub fn new(
        terminal: Terminal<B>,
        state: AppState,
        theme: Theme,
        hooks: Hooks,
        depth: ColorDepth,
        controls: Box<dyn Controls>,
        bell: Box<dyn FnMut() + Send>,
    ) -> Self {
        Runtime {
            state,
            terminal,
            theme,
            hooks,
            depth,
            controls,
            bell,
            drawn: None,
            draws: 0,
            spinner_at: None,
            exit: None,
        }
    }

    pub fn quit_requested(&self) -> bool {
        self.exit.is_some()
    }

    fn apply(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::Quit => self.exit = Some(0),
                Effect::Bell => (self.bell)(),
                Effect::PollNow => self.controls.poll_now(),
                Effect::SetPaused(paused) => self.controls.set_paused(paused),
                Effect::SetIntervalSeconds(seconds) => {
                    self.controls.set_interval_ms(u64::from(seconds) * 1000)
                }
                Effect::Call(action) => self.controls.call(action),
            }
        }
    }

    /// One event at time `now` (milliseconds since the epoch).
    pub fn handle(&mut self, event: Event, now: i64) {
        let effects = match event {
            Event::Input(TerminalEvent::Key(key)) => match keys::from_event(&key) {
                Some(key) => self.state.on_key(key, now),
                None => Vec::new(),
            },
            Event::Input(_) => Vec::new(),
            Event::Poll(poll) => self.state.on_poll(poll, now),
            Event::CallDone { action, result } => self.state.on_call_done(&action, result, now),
            Event::Signal(number) => {
                self.exit = Some(128 + number as u8);
                Vec::new()
            }
        };
        self.apply(effects);
    }

    /// The next moment something changes on its own: the clock second, the spinner frame, a notice ending.
    pub fn next_deadline(&self, now: i64) -> i64 {
        let clock = (now.div_euclid(1000) + 1) * 1000;
        [Some(clock), self.spinner_at, self.state.notice_deadline()]
            .into_iter()
            .flatten()
            .min()
            .unwrap_or(clock)
    }

    /// Runs what is due at `now`.
    pub fn on_time(&mut self, now: i64) {
        self.state.expire(now);
        if self.state.spinner_runs() {
            match self.spinner_at {
                Some(at) if now >= at => {
                    self.state.advance_spinner();
                    self.spinner_at = Some(now + SPINNER_MS);
                }
                Some(_) => {}
                None => self.spinner_at = Some(now + SPINNER_MS),
            }
        } else {
            self.spinner_at = None;
        }
    }

    fn size(&self) -> io::Result<Size> {
        let size = self.terminal.size()?;
        Ok(Size {
            columns: size.width,
            rows: size.height,
        })
    }

    /// Paints when what the screen shows has changed since the last paint; true when it painted.
    pub fn redraw_if_needed(&mut self, now: i64) -> io::Result<bool> {
        let size = self.size()?;
        let mut view = self.state.view_state(size, now, false);
        view.now_ms = 0;
        let key = DrawKey {
            view,
            overlay: self.state.overlay(),
            model_rev: self.state.model_rev,
            placeholder: self.state.placeholder(size),
        };
        if self.drawn.as_ref() == Some(&key) {
            return Ok(false);
        }
        self.paint(size, now, &key)?;
        self.drawn = Some(key);
        Ok(true)
    }

    fn paint(&mut self, size: Size, now: i64, key: &DrawKey) -> io::Result<()> {
        let depth = self.depth;
        let measure = self.hooks.cell_width;
        let area = Rect::new(0, 0, size.columns, size.rows);
        if let Some(text) = &key.placeholder {
            self.terminal
                .draw(|f| paint::paint_text(f.buffer_mut(), area, text, measure))?;
        } else if let Some(model) = &self.state.model {
            let view = self.state.view_state(size, now, true);
            let frame = (self.hooks.build_frame)(model, &view, &self.theme);
            let overlay: Option<Overlay> = match &key.overlay {
                Some(OverlayKind::Peek(peek)) => {
                    Some((self.hooks.observe_overlay)(peek, size, &self.theme))
                }
                Some(OverlayKind::Prompt(action)) => {
                    Some((self.hooks.confirm_overlay)(action, size, &self.theme))
                }
                Some(OverlayKind::Help) => Some((self.hooks.help_overlay)(size, &self.theme)),
                None => None,
            };
            self.terminal.draw(|f| {
                paint::paint_screen(
                    f.buffer_mut(),
                    area,
                    &frame.lines,
                    overlay.as_ref(),
                    depth,
                    measure,
                )
            })?;
            self.state.shown = frame.shown;
        }
        self.draws += 1;
        Ok(())
    }

    /// Runs until quit. `now` is the clock.
    pub fn run_loop(&mut self, events: &Receiver<Event>, now: &dyn Fn() -> i64) -> io::Result<()> {
        self.on_time(now());
        self.redraw_if_needed(now())?;
        while !self.quit_requested() {
            let at = now();
            let wait = (self.next_deadline(at) - at).max(0) as u64;
            match events.recv_timeout(Duration::from_millis(wait)) {
                Ok(event) => {
                    self.handle(event, now());
                    while let Ok(event) = events.try_recv() {
                        self.handle(event, now());
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            let at = now();
            self.on_time(at);
            if self.quit_requested() {
                break;
            }
            self.redraw_if_needed(at)?;
        }
        Ok(())
    }
}

/// The poller and the action calls over a real socket.
pub struct LiveControls {
    poller: Poller,
    client: Arc<Client>,
    events: Sender<Event>,
}

impl LiveControls {
    /// Starts polling `status` at once; its results arrive on `events`.
    pub fn start(
        client: Arc<Client>,
        interval_ms: u64,
        backoff: Vec<u64>,
        events: Sender<Event>,
    ) -> Self {
        let poll_client = Arc::clone(&client);
        let poll_events = events.clone();
        let poller = Poller::spawn(
            interval_ms,
            backoff,
            move || poll_client.status(),
            move |poll| {
                let _ = poll_events.send(Event::Poll(poll));
            },
        );
        LiveControls {
            poller,
            client,
            events,
        }
    }
}

impl Controls for LiveControls {
    fn poll_now(&self) {
        self.poller.poll_now();
    }
    fn set_paused(&self, paused: bool) {
        self.poller.set_paused(paused);
    }
    fn set_interval_ms(&self, interval_ms: u64) {
        self.poller.set_interval_ms(interval_ms);
    }
    fn call(&self, action: DashAction) {
        let client = Arc::clone(&self.client);
        let events = self.events.clone();
        std::thread::spawn(move || {
            let result = client.run_action(&action);
            let _ = events.send(Event::CallDone { action, result });
        });
    }
}

fn process_env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

fn fail(code: u8, text: &str) -> ExitCode {
    let _ = io::stderr().write_all(text.as_bytes());
    ExitCode::from(code)
}

pub fn run(args: Vec<String>) -> ExitCode {
    let options = match args::parse_args(&args) {
        Ok(options) => options,
        Err(error) => return fail(error.code, &error.render()),
    };
    let credential = match args::credential_from(std::env::var(args::CREDENTIAL_VARIABLE).ok()) {
        Ok(credential) => credential,
        Err(error) => return fail(error.code, &error.render()),
    };
    std::env::remove_var(args::CREDENTIAL_VARIABLE);
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return fail(
            args::EXIT_RUNTIME,
            &format!("cstan-dash: {}\n", terminal::NOT_A_TERMINAL_MESSAGE),
        );
    }
    match run_terminal(options, credential) {
        Ok(code) => ExitCode::from(code),
        Err(error) => fail(args::EXIT_RUNTIME, &format!("cstan-dash: {error}\n")),
    }
}

fn run_terminal(options: args::Options, credential: String) -> io::Result<u8> {
    let env = process_env;
    let theme = make_theme(ThemeOptions {
        no_color: terminal::wants_no_color(options.no_color, &env),
        reduced_motion: terminal::wants_reduced_motion(options.reduced_motion, &env),
        ascii: terminal::wants_ascii(&env),
    });
    let depth = ColorDepth::detect(
        process_env("COLORTERM").as_deref(),
        process_env("TERM").as_deref(),
    );
    let (sender, receiver) = mpsc::channel::<Event>();
    let client = Arc::new(Client::new(options.socket.clone(), credential));
    let mut session = terminal::Session::enter(terminal::CrosstermOps)?;
    terminal::install_panic_hook(terminal::restore_terminal);
    let backend = CrosstermBackend::new(io::stdout());
    let terminal = Terminal::new(backend)?;

    let controls = LiveControls::start(
        client,
        u64::from(options.interval_seconds) * 1000,
        BACKOFF_MS.to_vec(),
        sender.clone(),
    );
    let input_sender = sender.clone();
    std::thread::spawn(move || {
        while let Ok(event) = event::read() {
            if input_sender.send(Event::Input(event)).is_err() {
                break;
            }
        }
    });
    let mut signals = signal_hook::iterator::Signals::new(terminal::quit_signals())?;
    let signal_sender = sender.clone();
    std::thread::spawn(move || {
        for number in signals.forever() {
            if signal_sender.send(Event::Signal(number)).is_err() {
                break;
            }
        }
    });

    let reduced_motion = terminal::wants_reduced_motion(options.reduced_motion, &env);
    let state = AppState::new(
        Hooks::default(),
        options.interval_seconds,
        options.worker_limit,
        reduced_motion,
    );
    let mut runtime = Runtime::new(
        terminal,
        state,
        theme,
        Hooks::default(),
        depth,
        Box::new(controls),
        Box::new(|| {
            let mut out = io::stdout();
            let _ = out.write_all(b"\x07");
            let _ = out.flush();
        }),
    );
    let outcome = runtime.run_loop(&receiver, &now_ms);
    session.leave();
    outcome?;
    Ok(runtime.exit.unwrap_or(0))
}
