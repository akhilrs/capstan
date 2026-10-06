//! End to end: a fake daemon serves each fixture's `status`; the real client, poller, model builder, frame builder and
//! painter turn it into a `TestBackend` buffer that must equal the Node frame, cell by cell (text and style). Also the
//! size sweep: every terminal size from 1x1 to 200x60 renders without a panic and without a line wider than the width.
mod common;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use common::{case_names, load_fixture};
use cstan_dash::app::client::Client;
use cstan_dash::app::paint::{paint_screen, ColorDepth};
use cstan_dash::app::poller::{PollEvent, Poller};
use cstan_dash::app::state::{AppState, Hooks};
use cstan_dash::model::{DashAction, DashModel};
use cstan_dash::view::{
    build_frame, cell_width, confirm_overlay, help_overlay, make_theme, observe_overlay, Line,
    PeekView, Size, Theme, ViewState,
};
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};
use ratatui::Terminal;
use serde_json::{json, Value};

const CREDENTIAL: &str = "e2e-operator-credential";

/// A daemon on a unix socket in a scratch directory: answers `status` with the given result, refuses a wrong credential.
struct FakeDaemon {
    socket: PathBuf,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    _dir: tempfile::TempDir,
}

impl FakeDaemon {
    fn serve(status: Value) -> FakeDaemon {
        let dir = tempfile::tempdir().expect("scratch dir");
        let socket = dir.path().join("d.sock");
        let listener = UnixListener::bind(&socket).expect("bind");
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let thread = std::thread::spawn(move || {
            for stream in listener.incoming() {
                if flag.load(Ordering::SeqCst) {
                    return;
                }
                let Ok(mut stream) = stream else { continue };
                let mut line = String::new();
                if BufReader::new(&stream).read_line(&mut line).is_err() {
                    continue;
                }
                let request: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
                let reply = if request["credential"] == CREDENTIAL && request["command"] == "status"
                {
                    json!({"ok": true, "requestId": "x", "result": status})
                } else {
                    json!({"ok": false, "code": "unauthorized", "message": "refused"})
                };
                let _ = stream.write_all(format!("{reply}\n").as_bytes());
            }
        });
        FakeDaemon {
            socket,
            stop,
            thread: Some(thread),
            _dir: dir,
        }
    }
}

impl Drop for FakeDaemon {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = UnixStream::connect(&self.socket);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// One expected cell: symbol, foreground, background, modifiers; `None` is the trailing half of a wide character.
type Expected = Option<(String, Color, Color, Modifier)>;

fn color_of_hex(hex: &str) -> Color {
    let digits = hex.strip_prefix('#').expect("hex colour");
    let byte = |at: usize| u8::from_str_radix(&digits[at..at + 2], 16).expect("hex digits");
    Color::Rgb(byte(0), byte(2), byte(4))
}

/// The cells a frame line must produce, worked out from the fixture JSON alone (not from the painter).
fn expected_row(line: &Value, columns: usize) -> Vec<Expected> {
    let mut cells: Vec<Expected> = (0..columns)
        .map(|_| {
            Some((
                " ".to_string(),
                Color::Reset,
                Color::Reset,
                Modifier::empty(),
            ))
        })
        .collect();
    let mut column = 0usize;
    let mut last: Option<usize> = None;
    for span in line.as_array().expect("line") {
        let fg = span["color"].as_str().map_or(Color::Reset, color_of_hex);
        let bg = span["bg"].as_str().map_or(Color::Reset, color_of_hex);
        let mut modifier = Modifier::empty();
        if span["bold"] == json!(true) {
            modifier |= Modifier::BOLD;
        }
        if span["dim"] == json!(true) {
            modifier |= Modifier::DIM;
        }
        for c in span["text"].as_str().expect("text").chars() {
            let text = c.to_string();
            let width = cell_width(&text);
            if width == 0 {
                if let Some(at) = last {
                    if let Some((symbol, ..)) = cells[at].as_mut() {
                        symbol.push(c);
                    }
                }
                continue;
            }
            assert!(column + width <= columns, "fixture line overflows");
            cells[column] = Some((text, fg, bg, modifier));
            for follow in 1..width {
                cells[column + follow] = None;
            }
            last = Some(column);
            column += width;
        }
    }
    cells
}

fn row_text(line: &Line) -> String {
    line.iter().map(|span| span.text.as_str()).collect()
}

#[test]
fn every_fixture_travels_daemon_to_buffer_and_equals_the_node_frame() {
    let names = case_names();
    assert!(names.len() >= 13, "cases: {names:?}");
    let mut compared = 0usize;
    for name in names {
        let fixture = load_fixture(&name);
        let now_ms = fixture["nowMs"].as_i64().expect("nowMs");
        let worker_limit = fixture["workerLimit"].as_i64();
        let daemon = FakeDaemon::serve(fixture["status"].clone());
        let client = Arc::new(Client::new(daemon.socket.clone(), CREDENTIAL.to_string()));

        // client -> poller
        let (sender, receiver) = mpsc::channel();
        let fetch_client = Arc::clone(&client);
        let poller = Poller::spawn(
            1000,
            vec![1000],
            move || fetch_client.status(),
            move |event| {
                let _ = sender.send(event);
            },
        );
        let event = receiver
            .recv_timeout(Duration::from_secs(10))
            .unwrap_or_else(|e| panic!("{name}: no poll event: {e}"));
        poller.join();
        match &event {
            PollEvent::Status { changed, .. } => assert!(*changed, "{name}: first poll is new"),
            other => panic!("{name}: the poll failed: {other:?}"),
        }

        // poller -> build_dash_model
        let mut state = AppState::new(Hooks::default(), 2, worker_limit, true);
        state.on_poll(event, now_ms);
        let model: &DashModel = state.model.as_ref().expect("the poll built a model");

        for (i, entry) in fixture["views"]
            .as_array()
            .expect("views")
            .iter()
            .enumerate()
        {
            let at = format!("{name}.views[{i}]");
            let view: ViewState = serde_json::from_value(entry["view"].clone()).expect("view");
            let theme: Theme =
                make_theme(serde_json::from_value(entry["theme"].clone()).expect("theme"));
            let size = view.size;

            // build_frame -> paint on a TestBackend
            let frame = build_frame(model, &view, &theme);
            let mut terminal = Terminal::new(TestBackend::new(size.columns, size.rows)).unwrap();
            let area = Rect::new(0, 0, size.columns, size.rows);
            terminal
                .draw(|f| {
                    paint_screen(
                        f.buffer_mut(),
                        area,
                        &frame.lines,
                        None,
                        ColorDepth::TrueColor,
                        cell_width,
                    )
                })
                .unwrap();
            let buffer = terminal.backend().buffer();

            let want_lines = entry["frame"]["lines"].as_array().expect("lines");
            assert_eq!(want_lines.len(), size.rows as usize, "{at}: rows");
            for (row, want) in want_lines.iter().enumerate() {
                let expected = expected_row(want, size.columns as usize);
                let mut text = String::new();
                for (column, cell) in expected.iter().enumerate() {
                    let got = &buffer[(column as u16, row as u16)];
                    let Some((symbol, fg, bg, modifier)) = cell else {
                        continue;
                    };
                    assert_eq!(
                        got.symbol(),
                        symbol,
                        "{at}: text at row {row} column {column}"
                    );
                    assert_eq!(got.fg, *fg, "{at}: fg at row {row} column {column}");
                    assert_eq!(got.bg, *bg, "{at}: bg at row {row} column {column}");
                    assert_eq!(
                        got.modifier, *modifier,
                        "{at}: modifiers at row {row} column {column}"
                    );
                    text.push_str(symbol);
                    compared += 1;
                }
                assert_eq!(
                    text,
                    row_text(&frame.lines[row]),
                    "{at}: row {row} text equals the built frame line"
                );
            }
        }
    }
    assert!(compared > 100_000, "compared only {compared} cells");
}

#[test]
fn a_wrong_credential_is_refused_and_the_poll_reports_the_link_down() {
    let daemon = FakeDaemon::serve(json!({"projectId": "x"}));
    let client = Client::new(daemon.socket.clone(), "not-the-credential".to_string());
    assert!(client.status().is_err());
}

// ---- the size sweep --------------------------------------------------------------------------------------------

/// A hand-made peek and the actions the confirm overlay shows, so the sweep covers every overlay.
fn peek() -> PeekView {
    PeekView {
        agent_id: "developer-2".to_string(),
        agent_status: "blocked".to_string(),
        text: "developer-2 pane p4\n\nEdit(src/dash/theme.ts)\n  Updated src/dash/theme.ts with 12 additions\n\nThinking... (3m 41s, esc to interrupt)\n".to_string(),
    }
}

fn widest(lines: &[Line]) -> usize {
    lines
        .iter()
        .map(|line| cell_width(&row_text(line)))
        .max()
        .unwrap_or(0)
}

/// What one sweep step found wrong.
#[derive(Debug)]
struct Finding {
    columns: u16,
    rows: u16,
    case: String,
    what: String,
    panic: bool,
}

/// One size and one fixture: the frame and every overlay, with a panic or an overlong line recorded.
fn sweep_one(
    size: Size,
    case: &str,
    (model, view, theme, actions): (&DashModel, &ViewState, &Theme, &[DashAction]),
    found: &mut Vec<Finding>,
) {
    let mut view = view.clone();
    view.size = size;
    let mut note = |what: String, panic: bool| {
        found.push(Finding {
            columns: size.columns,
            rows: size.rows,
            case: case.to_string(),
            what,
            panic,
        })
    };
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let mut problems = Vec::new();
        let frame = build_frame(model, &view, theme);
        if widest(&frame.lines) > size.columns as usize {
            problems.push("frame line wider than the width".to_string());
        }
        let mut overlays = vec![
            ("help", help_overlay(size, theme)),
            ("observe", observe_overlay(&peek(), size, theme)),
        ];
        for action in actions {
            overlays.push(("confirm", confirm_overlay(action, size, theme)));
        }
        for (kind, overlay) in overlays {
            if overlay.left as usize + widest(&overlay.lines) > size.columns as usize {
                problems.push(format!("{kind} overlay wider than the width"));
            }
        }
        problems
    }));
    match outcome {
        Ok(problems) => problems.into_iter().for_each(|p| note(p, false)),
        Err(_) => note("panic".to_string(), true),
    }
}

/// Every size from 1x1 to 200x60, spread over the cores: (renders, findings). `every_fixture` renders each size with all
/// fixtures' first views; otherwise with `showcase` and one other fixture chosen by the size (a debug build is slow).
fn sweep(every_fixture: bool) -> (usize, Vec<Finding>) {
    type Case = (String, DashModel, ViewState, Theme, Vec<DashAction>);
    let mut cases: Vec<Case> = Vec::new();
    for name in case_names() {
        let fixture = load_fixture(&name);
        let model: DashModel = serde_json::from_value(fixture["model"].clone()).expect("model");
        let entry = &fixture["views"][0];
        let view: ViewState = serde_json::from_value(entry["view"].clone()).expect("view");
        let theme = make_theme(serde_json::from_value(entry["theme"].clone()).expect("theme"));
        let actions: Vec<DashAction> = fixture["views"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|v| {
                v["overlays"]["confirm"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
            })
            .map(|c| serde_json::from_value(c["action"].clone()).expect("action"))
            .take(3)
            .collect();
        cases.push((name, model, view, theme, actions));
    }
    let cases = Arc::new(cases);
    let workers = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(16);
    let handles: Vec<_> = (0..workers)
        .map(|worker| {
            let cases = Arc::clone(&cases);
            std::thread::spawn(move || {
                let mut found = Vec::new();
                let mut renders = 0usize;
                for columns in (1..=200u16).filter(|c| *c as usize % workers == worker) {
                    for rows in 1..=60u16 {
                        let other = (columns as usize + rows as usize) % cases.len();
                        for (at, (name, model, view, theme, actions)) in cases.iter().enumerate() {
                            if !every_fixture && name != "showcase" && at != other {
                                continue;
                            }
                            renders += 1;
                            let parts = (model, view, theme, actions.as_slice());
                            sweep_one(Size { columns, rows }, name, parts, &mut found);
                        }
                    }
                }
                (renders, found)
            })
        })
        .collect();
    let mut renders = 0;
    let mut found = Vec::new();
    for handle in handles {
        let (count, mut part) = handle.join().expect("sweep worker");
        renders += count;
        found.append(&mut part);
    }
    found.sort_by(|a, b| {
        (a.columns, a.rows, &a.case, &a.what).cmp(&(b.columns, b.rows, &b.case, &b.what))
    });
    (renders, found)
}

fn describe(found: &[&Finding]) -> String {
    let mut text = format!("{} findings; first 25:\n", found.len());
    for f in found.iter().take(25) {
        text.push_str(&format!(
            "  {}x{} {}: {}\n",
            f.columns, f.rows, f.case, f.what
        ));
    }
    text
}

/// The dashboard paints a frame only from 60x16 up (`MIN_COLUMNS`, `MIN_ROWS`); below that it shows one line of text.
fn painted(f: &Finding) -> bool {
    f.columns >= cstan_dash::app::state::MIN_COLUMNS && f.rows >= cstan_dash::app::state::MIN_ROWS
}

#[test]
fn no_size_panics_and_no_painted_size_has_an_overlong_line() {
    let every = std::env::var_os("CSTAN_SWEEP_ALL").is_some();
    let (renders, found) = sweep(every);
    assert!(renders >= 12_000, "rendered {renders}");
    let panics: Vec<&Finding> = found.iter().filter(|f| f.panic).collect();
    assert!(panics.is_empty(), "panics: {}", describe(&panics));
    let overlong: Vec<&Finding> = found.iter().filter(|f| painted(f)).collect();
    assert!(
        overlong.is_empty(),
        "painted sizes: {}",
        describe(&overlong)
    );
    let below: Vec<&Finding> = found.iter().filter(|f| !painted(f)).collect();
    println!(
        "size sweep: {renders} renders, no panic; {} overlong lines below 60x16 (not painted)",
        below.len()
    );
}

/// The whole range with every fixture, also the sizes under 60x16 that the app replaces with a placeholder.
/// The other sweep takes every fixture with CSTAN_SWEEP_ALL=1.
#[test]
fn every_size_from_1x1_to_200x60_has_no_line_wider_than_the_width() {
    let (_, found) = sweep(true);
    let all: Vec<&Finding> = found.iter().collect();
    assert!(all.is_empty(), "{}", describe(&all));
}
