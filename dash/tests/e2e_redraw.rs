//! End to end: what the real runtime (real model and view, a crossterm backend writing into a byte counter) writes
//! to the terminal when only the spinner moves, and that unchanged polls do not draw.
mod common;

use std::io::{self, Write};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::load_fixture;
use cstan_dash::app::paint::ColorDepth;
use cstan_dash::app::poller::{status_hash, PollEvent, Poller};
use cstan_dash::app::state::{AppState, Hooks};
use cstan_dash::app::{Event, NoControls, Runtime};
use cstan_dash::view::{make_theme, ThemeOptions};
use ratatui::backend::{Backend, ClearType, CrosstermBackend, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};
use ratatui::Terminal;

const COLUMNS: u16 = 160;
const ROWS: u16 = 45;
/// Mid-second, so a spinner tick (120 ms) does not cross a clock-second boundary.
const NOW: i64 = 1_790_942_400_000 + 100;

#[derive(Default)]
struct Tally {
    bytes: usize,
    cells: Vec<(u16, u16, String)>,
}

/// Counts the bytes the terminal would receive.
struct Sink(Arc<Mutex<Tally>>);

impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).bytes += buf.len();
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The crossterm backend over a counting sink, with a fixed size (there is no tty) and a record of the cells it draws.
struct CountingBackend {
    inner: CrosstermBackend<Sink>,
    tally: Arc<Mutex<Tally>>,
}

impl CountingBackend {
    fn new() -> (Self, Arc<Mutex<Tally>>) {
        let tally = Arc::new(Mutex::new(Tally::default()));
        let backend = CountingBackend {
            inner: CrosstermBackend::new(Sink(Arc::clone(&tally))),
            tally: Arc::clone(&tally),
        };
        (backend, tally)
    }
}

impl Backend for CountingBackend {
    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        let cells: Vec<(u16, u16, &Cell)> = content.collect();
        {
            let mut tally = self.tally.lock().unwrap();
            for (x, y, cell) in &cells {
                tally.cells.push((*x, *y, cell.symbol().to_string()));
            }
        }
        self.inner.draw(cells.into_iter())
    }
    fn hide_cursor(&mut self) -> io::Result<()> {
        self.inner.hide_cursor()
    }
    fn show_cursor(&mut self) -> io::Result<()> {
        self.inner.show_cursor()
    }
    fn get_cursor_position(&mut self) -> io::Result<Position> {
        Ok(Position { x: 0, y: 0 })
    }
    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        self.inner.set_cursor_position(position)
    }
    fn clear(&mut self) -> io::Result<()> {
        self.inner.clear()
    }
    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        self.inner.clear_region(clear_type)
    }
    fn size(&self) -> io::Result<Size> {
        Ok(Size::new(COLUMNS, ROWS))
    }
    fn window_size(&mut self) -> io::Result<WindowSize> {
        Ok(WindowSize {
            columns_rows: Size::new(COLUMNS, ROWS),
            pixels: Size::new(0, 0),
        })
    }
    fn flush(&mut self) -> io::Result<()> {
        Backend::flush(&mut self.inner)
    }
}

/// A runtime on the real model and view, showing the `tasks` fixture (4 agents working), already painted once.
fn running() -> (
    Runtime<CountingBackend>,
    Arc<Mutex<Tally>>,
    serde_json::Value,
) {
    let fixture = load_fixture("tasks");
    let status = fixture["status"].clone();
    let (backend, tally) = CountingBackend::new();
    let terminal = Terminal::new(backend).unwrap();
    let state = AppState::new(Hooks::default(), 2, fixture["workerLimit"].as_i64(), false);
    let theme = make_theme(ThemeOptions::default());
    let mut runtime = Runtime::new(
        terminal,
        state,
        theme,
        Hooks::default(),
        ColorDepth::TrueColor,
        Box::new(NoControls),
        Box::new(|| {}),
    );
    runtime.handle(
        Event::Poll(PollEvent::Status {
            status: status.clone(),
            changed: true,
        }),
        NOW,
    );
    runtime.on_time(NOW);
    assert!(runtime.redraw_if_needed(NOW).unwrap(), "first paint");
    (runtime, tally, status)
}

#[test]
fn one_spinner_tick_writes_only_spinner_cells_in_at_most_200_bytes() {
    let (mut runtime, tally, _) = running();
    let working = runtime.state.model.as_ref().unwrap().counts.working;
    assert_eq!(working, 4, "the fixture has 4 working agents");
    assert!(runtime.state.spinner_runs());

    // The first paint wrote the whole screen; that is what a tick must not repeat.
    let full = tally.lock().unwrap().bytes;
    assert!(full > 1000, "first paint wrote {full} bytes");

    let mut worst = 0usize;
    let mut bar_cells = 0usize;
    let mut ticks = 0;
    let mut now = NOW;
    // Six ticks of 120 ms from 100 ms past the second stay inside the same clock second.
    for _ in 0..6 {
        {
            let mut t = tally.lock().unwrap();
            t.bytes = 0;
            t.cells.clear();
        }
        now += 120;
        assert_eq!(now.div_euclid(1000), NOW.div_euclid(1000));
        runtime.on_time(now);
        let drawn = runtime.redraw_if_needed(now).unwrap();
        let t = tally.lock().unwrap();
        assert!(drawn, "a spinner tick draws");
        assert!(!t.cells.is_empty());
        let spinner: Vec<&str> = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"].into();
        // The recency bar of an active agent (8 cells over 30 s) steps by itself: a cell of it may change in a tick.
        let bar = ["█", "░", "▏", "▎", "▍", "▌", "▋", "▊", "▉"];
        let mut spinner_cells = 0;
        for (x, y, symbol) in &t.cells {
            if spinner.contains(&symbol.as_str()) {
                spinner_cells += 1;
            } else if bar.contains(&symbol.as_str()) {
                bar_cells += 1;
            } else {
                panic!(
                    "cell ({x},{y}) {symbol:?} is neither a spinner glyph nor a recency-bar cell"
                );
            }
        }
        assert_eq!(
            spinner_cells, working as usize,
            "one spinner cell per working agent changes"
        );
        assert!(t.bytes <= 200, "a spinner tick wrote {} bytes", t.bytes);
        worst = worst.max(t.bytes);
        ticks += 1;
        println!(
            "spinner tick {ticks}: {} bytes, {} cells changed (first paint {full} bytes)",
            t.bytes,
            t.cells.len()
        );
    }
    assert_eq!(ticks, 6);
    println!("RECORDED recency-bar cells that stepped during the 6 ticks: {bar_cells}");
    println!(
        "RECORDED worst spinner tick: {worst} bytes at {COLUMNS}x{ROWS} with 4 working agents"
    );
}

#[test]
fn a_hundred_unchanged_polls_cause_no_draw() {
    let (mut runtime, tally, status) = running();
    {
        let mut t = tally.lock().unwrap();
        t.bytes = 0;
        t.cells.clear();
    }
    let draws = runtime.draws;
    let rev = runtime.state.model_rev;

    // The real poller over the same status: the first result is new, the following ones are not.
    let (sender, receiver) = mpsc::channel();
    let fetched = status.clone();
    let poller = Poller::spawn(
        1,
        vec![1],
        move || Ok(fetched.clone()),
        move |event| {
            let _ = sender.send(event);
        },
    );
    let mut unchanged = 0;
    let mut first = true;
    while unchanged < 100 {
        let event = receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("the poller keeps polling");
        let PollEvent::Status {
            changed,
            status: polled,
        } = &event
        else {
            panic!("poll failed: {event:?}");
        };
        assert_eq!(status_hash(polled), status_hash(&status));
        if first {
            // The poller has not seen this status yet: it reports a change; the runtime already shows it.
            assert!(*changed);
            first = false;
            continue;
        }
        assert!(!*changed, "unchanged status reported as changed");
        runtime.handle(Event::Poll(event), NOW);
        runtime.on_time(NOW);
        assert!(
            !runtime.redraw_if_needed(NOW).unwrap(),
            "poll {unchanged} drew"
        );
        unchanged += 1;
    }
    poller.join();
    assert_eq!(runtime.draws, draws, "no draw for 100 unchanged polls");
    assert_eq!(runtime.state.model_rev, rev, "no model rebuild");
    assert_eq!(tally.lock().unwrap().bytes, 0, "nothing written");
}

#[test]
fn a_spinner_tick_paints_only_the_changed_lines_into_the_buffer() {
    let (mut runtime, _, _) = running();
    assert_eq!(
        runtime.rows_painted, ROWS as usize,
        "the first frame paints every line"
    );
    let working = runtime.state.model.as_ref().unwrap().counts.working as usize;
    let mut now = NOW;
    for _ in 0..6 {
        now += 120;
        runtime.on_time(now);
        assert!(runtime.redraw_if_needed(now).unwrap());
        // One line per working agent carries a spinner; a recency-bar step may sit on the same line.
        assert!(
            runtime.rows_painted >= 1 && runtime.rows_painted <= working,
            "a tick painted {} lines",
            runtime.rows_painted
        );
    }
}

#[test]
fn painting_changed_rows_gives_the_same_buffer_as_painting_the_whole_screen() {
    use cstan_dash::app::paint::{paint_rows, paint_screen};
    use cstan_dash::view::{Line, Overlay, Span};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    let line = |text: &str| -> Line {
        vec![Span {
            text: text.to_string(),
            ..Span::default()
        }]
    };
    let area = Rect::new(0, 0, 20, 4);
    let before = vec![line("alpha long line"), line("b"), line("c"), line("d")];
    let after = vec![
        line("alpha long line"),
        line("short"),
        line("c"),
        line("d!"),
    ];
    let overlay = Overlay {
        lines: vec![line("[box]")],
        top: 1,
        left: 3,
    };
    let width = cstan_dash::view::cell_width;
    let depth = ColorDepth::TrueColor;
    let mut partial = Buffer::empty(area);
    paint_screen(&mut partial, area, &before, Some(&overlay), depth, width);
    paint_rows(
        &mut partial,
        area,
        &[1, 3],
        &after,
        Some(&overlay),
        depth,
        width,
    );
    let mut whole = Buffer::empty(area);
    paint_screen(&mut whole, area, &after, Some(&overlay), depth, width);
    assert_eq!(partial, whole);
}
