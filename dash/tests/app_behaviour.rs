//! The dashboard runtime against a fake daemon on a unix socket and a fake frame source on a ratatui `TestBackend`.
mod common;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossterm::event::{Event as TerminalEvent, KeyCode, KeyEvent, KeyModifiers};
use cstan_dash::app::args::{
    credential_from, parse_args, ArgError, EXIT_INTERVAL, EXIT_USAGE, NO_CREDENTIAL_MESSAGE, USAGE,
};
use cstan_dash::app::client::{
    CallResult, Client, ClientError, Response, MAX_FRAME_BYTES, MAX_RESPONSE_BYTES,
};
use cstan_dash::app::keys::{self, Key};
use cstan_dash::app::paint::{color_of, paint_line, paint_screen, ColorDepth};
use cstan_dash::app::poller::{
    next_delay_ms, status_hash, step_interval, PollEvent, Poller, BACKOFF_MS,
};
use cstan_dash::app::state::{
    resolve_selection, row_ids, signals_of, AppState, Hooks, Selection, RING_LIMIT,
};
use cstan_dash::app::terminal::{
    is_utf8_locale, wants_ascii, wants_no_color, wants_reduced_motion, Session, TerminalOps,
};
use cstan_dash::app::{Controls, Event, LiveControls, Runtime};
use cstan_dash::model::{confirm_text, DashAction, DashModel};
use cstan_dash::view::{Frame, Link, Overlay, PanelId, Size, Span, Theme, ViewState};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};
use ratatui::Terminal;
use serde_json::{json, Value};

const NOW: i64 = 1_700_000_000_000;
const COLUMNS: u16 = 100;
const ROWS: u16 = 30;

// ---- the fake frame source -------------------------------------------------------------------------------------

fn width_of(text: &str) -> usize {
    text.chars()
        .map(|c| match c as u32 {
            0x300..=0x36f => 0,
            0x1100..=0x115f | 0x2e80..=0xa4cf | 0xac00..=0xd7a3 | 0xff00..=0xff60 => 2,
            _ => 1,
        })
        .sum()
}

fn plain(text: impl Into<String>) -> Vec<Span> {
    vec![Span {
        text: text.into(),
        ..Span::default()
    }]
}

const SPINNER: [&str; 4] = ["|", "/", "-", "\\"];

fn fake_model(status: &Value, _now: i64, _limit: Option<i64>) -> DashModel {
    serde_json::from_value(status.clone()).expect("the fake status is a model")
}

fn fake_frame(model: &DashModel, view: &ViewState, _theme: &Theme) -> Frame {
    let mut lines = vec![
        plain(format!(
            "{} link={:?} paused={} focus={:?} every={}s age={}",
            view.clock, view.link, view.paused, view.focus, view.interval_seconds, view.link_age
        )),
        plain(view.notice.clone().unwrap_or_default()),
        vec![
            Span {
                text: SPINNER[(view.tick % 4) as usize].to_string(),
                color: Some("#77ca9b".to_string()),
                bold: Some(true),
                ..Span::default()
            },
            Span {
                text: format!(" working={}", model.counts.working),
                ..Span::default()
            },
        ],
        plain(format!(
            "sel a={} p={} q={} f={} w={} problems={} ended={} rings={} hl={}",
            view.selected.agents,
            view.selected.pipeline,
            view.selected.queue,
            view.selected.findings,
            view.selected.work,
            view.problems_only,
            view.show_all_ended,
            view.rings.working.len(),
            view.highlight.len()
        )),
    ];
    for message in &model.queue.messages {
        lines.push(plain(format!(
            "msg {} {}",
            message.message_id, message.state
        )));
    }
    Frame {
        lines,
        shown: vec![
            PanelId::Agents,
            PanelId::Pipeline,
            PanelId::Queue,
            PanelId::Findings,
            PanelId::Work,
        ],
    }
}

/// Overlay lines wrapped at 60 characters, as a real overlay is a fixed-width box.
fn fake_overlay(lines: Vec<String>) -> Overlay {
    let mut wrapped = Vec::new();
    for line in lines {
        let mut row = String::new();
        for word in line.split(' ') {
            if !row.is_empty() && row.len() + 1 + word.len() > 60 {
                wrapped.push(std::mem::take(&mut row));
            }
            if !row.is_empty() {
                row.push(' ');
            }
            row.push_str(word);
        }
        wrapped.push(row);
    }
    Overlay {
        lines: wrapped.into_iter().map(plain).collect(),
        top: 10,
        left: 5,
    }
}

fn fake_hooks() -> Hooks {
    Hooks {
        build_model: fake_model,
        build_frame: fake_frame,
        help_overlay: |_, _| fake_overlay(vec!["HELP overlay".to_string()]),
        confirm_overlay: |action, _, _| fake_overlay(vec![confirm_text(action)]),
        observe_overlay: |peek, _, _| {
            fake_overlay(vec![
                format!("observe {} ({})", peek.agent_id, peek.agent_status),
                peek.text.clone(),
            ])
        },
        cell_width: width_of,
    }
}

// ---- the harness -----------------------------------------------------------------------------------------------

#[derive(Clone, Default)]
struct Recorder {
    log: Arc<Mutex<Vec<String>>>,
    calls: Arc<Mutex<Vec<DashAction>>>,
}

impl Recorder {
    fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
    fn calls(&self) -> Vec<DashAction> {
        self.calls.lock().unwrap().clone()
    }
}

struct FakeControls(Recorder);

impl Controls for FakeControls {
    fn poll_now(&self) {
        self.0.log.lock().unwrap().push("poll_now".to_string());
    }
    fn set_paused(&self, paused: bool) {
        self.0.log.lock().unwrap().push(format!("paused {paused}"));
    }
    fn set_interval_ms(&self, interval_ms: u64) {
        self.0
            .log
            .lock()
            .unwrap()
            .push(format!("interval {interval_ms}"));
    }
    fn call(&self, action: DashAction) {
        self.0.calls.lock().unwrap().push(action);
    }
}

fn showcase() -> Value {
    common::load_fixture("showcase")["model"].clone()
}

struct Harness {
    rt: Runtime<TestBackend>,
    recorder: Recorder,
    bells: Arc<AtomicUsize>,
    now: i64,
}

fn key(code: KeyCode) -> Event {
    Event::Input(TerminalEvent::Key(KeyEvent::new(code, KeyModifiers::NONE)))
}

fn ch(c: char) -> Event {
    key(KeyCode::Char(c))
}

fn harness_with(reduced_motion: bool, confirm_delay_ms: i64) -> Harness {
    let recorder = Recorder::default();
    let bells = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&bells);
    let mut state = AppState::new(fake_hooks(), 2, Some(3), reduced_motion);
    state.confirm_delay_ms = confirm_delay_ms;
    let terminal = Terminal::new(TestBackend::new(COLUMNS, ROWS)).unwrap();
    let theme = Theme {
        reduced_motion,
        ..Theme::default()
    };
    let rt = Runtime::new(
        terminal,
        state,
        theme,
        fake_hooks(),
        ColorDepth::TrueColor,
        Box::new(FakeControls(recorder.clone())),
        Box::new(move || {
            counted.fetch_add(1, Ordering::SeqCst);
        }),
    );
    let mut harness = Harness {
        rt,
        recorder,
        bells,
        now: NOW,
    };
    harness.rt.on_time(NOW);
    harness.rt.redraw_if_needed(NOW).unwrap();
    harness
}

fn harness() -> Harness {
    harness_with(false, 300)
}

impl Harness {
    fn send(&mut self, event: Event) {
        self.rt.handle(event, self.now);
        self.rt.on_time(self.now);
        self.rt.redraw_if_needed(self.now).unwrap();
    }
    fn advance(&mut self, ms: i64) {
        self.now += ms;
        self.rt.on_time(self.now);
        self.rt.redraw_if_needed(self.now).unwrap();
    }
    fn key(&mut self, c: char) {
        self.send(ch(c));
    }
    fn status(&mut self, status: &Value) {
        self.send(Event::Poll(PollEvent::Status {
            status: status.clone(),
            changed: true,
        }));
    }
    fn unchanged(&mut self, status: &Value) {
        self.send(Event::Poll(PollEvent::Status {
            status: status.clone(),
            changed: false,
        }));
    }
    fn fail(&mut self, link: Link) {
        self.send(Event::Poll(PollEvent::Failed { link }));
    }
    fn rows(&self) -> Vec<String> {
        screen(self.rt.terminal.backend().buffer())
    }
    fn text(&self) -> String {
        self.rows().join("\n")
    }
    /// The overlay rows (from row 10 down) as one line of single-spaced words.
    fn overlay_text(&self) -> String {
        self.rows()[10..]
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }
    fn up(&mut self) {
        self.status(&showcase());
    }
    /// The message the queue selection is on at the start: the first one.
    fn selected_message(&self) -> cstan_dash::model::MessageRow {
        let model = self.rt.state.model.as_ref().unwrap();
        cstan_dash::model::queue_rows(model, self.rt.state.problems_only)[0].clone()
    }
}

fn screen(buffer: &Buffer) -> Vec<String> {
    let area = buffer.area;
    (0..area.height)
        .map(|y| {
            (0..area.width)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

// ---- link states, placeholders, redraw ---------------------------------------------------------------------------

#[test]
fn link_goes_starting_ok_down_toolarge_and_the_last_frame_stays() {
    let mut h = harness();
    assert!(h.text().contains("connecting to the controller..."));
    h.up();
    assert!(h.text().contains("link=Ok"));
    h.fail(Link::Down);
    assert!(h.text().contains("link=Down"));
    assert!(
        h.text().contains("msg "),
        "the last frame stays while the link is down"
    );
    h.fail(Link::Toolarge);
    assert!(h.text().contains("link=Toolarge"));
    h.up();
    assert!(h.text().contains("link=Ok"));
}

#[test]
fn before_the_first_answer_a_down_link_says_the_controller_is_not_answering() {
    let mut h = harness();
    h.fail(Link::Down);
    assert!(h.text().contains("controller not answering, retrying"));
}

#[test]
fn a_terminal_below_60_by_16_says_it_is_too_small() {
    let mut h = harness();
    h.up();
    h.rt.terminal.backend_mut().resize(50, 12);
    h.advance(0);
    assert!(h
        .text()
        .contains("terminal too small (need 60x16, have 50x12)"));
}

#[test]
fn an_unchanged_poll_neither_rebuilds_the_model_nor_draws() {
    let mut h = harness();
    h.up();
    let (draws, rev) = (h.rt.draws, h.rt.state.model_rev);
    h.unchanged(&showcase());
    h.unchanged(&showcase());
    assert_eq!(h.rt.draws, draws, "no draw");
    assert_eq!(h.rt.state.model_rev, rev, "no rebuild");
    let mut other = showcase();
    other["counts"]["unresolved"] = json!(9);
    h.status(&other);
    assert_eq!(h.rt.state.model_rev, rev + 1);
    assert!(h.rt.draws > draws);
}

#[test]
fn a_spinner_tick_changes_only_the_spinner_cell() {
    let mut h = harness();
    h.up();
    let before = h.rt.terminal.backend().buffer().clone();
    h.advance(120);
    h.advance(0);
    let after = h.rt.terminal.backend().buffer().clone();
    assert_ne!(before, after);
    let changed: Vec<(u16, u16)> = before
        .diff(&after)
        .iter()
        .map(|(x, y, _)| (*x, *y))
        .collect();
    assert_eq!(changed, vec![(0, 2)], "only the spinner cell is repainted");
}

#[test]
fn the_spinner_stops_for_reduced_motion_and_when_nothing_works() {
    let mut h = harness_with(true, 300);
    h.up();
    let draws = h.rt.draws;
    h.advance(120);
    h.advance(120);
    assert_eq!(
        h.rt.draws, draws,
        "reduced motion: no spinner tick, no draw"
    );
    let mut idle = harness();
    let mut model = showcase();
    model["counts"]["working"] = json!(0);
    idle.status(&model);
    let draws = idle.rt.draws;
    idle.advance(120);
    idle.advance(120);
    assert_eq!(idle.rt.draws, draws);
}

#[test]
fn the_clock_second_redraws_and_the_next_wake_is_the_earliest_timer() {
    let mut h = harness_with(true, 300);
    h.up();
    let draws = h.rt.draws;
    h.advance(10);
    assert_eq!(h.rt.draws, draws, "same second");
    let next = h.rt.next_deadline(h.now);
    assert_eq!(next, (h.now.div_euclid(1000) + 1) * 1000);
    h.advance(1000);
    assert!(h.rt.draws > draws, "a new second repaints the clock");
    let mut spinning = harness();
    spinning.up();
    assert_eq!(spinning.rt.next_deadline(NOW), NOW + 120);
}

#[test]
fn a_notice_lasts_five_seconds() {
    let mut h = harness_with(true, 300);
    h.up();
    let start = h.now;
    h.rt.state.set_notice("hello notice", start);
    h.advance(0);
    assert!(h.text().contains("hello notice"));
    assert_eq!(h.rt.state.notice_deadline(), Some(start + 5000));
    h.advance(4999);
    assert!(h.text().contains("hello notice"));
    h.advance(1);
    assert!(!h.text().contains("hello notice"));
    assert_eq!(h.rt.state.notice_deadline(), None);
}

// ---- keys --------------------------------------------------------------------------------------------------------

#[test]
fn number_keys_tab_and_shift_tab_move_the_focus() {
    let mut h = harness();
    h.up();
    assert_eq!(h.rt.state.focus, PanelId::Queue);
    h.key('1');
    assert_eq!(h.rt.state.focus, PanelId::Agents);
    h.key('5');
    assert_eq!(h.rt.state.focus, PanelId::Work);
    h.send(key(KeyCode::Tab));
    assert_eq!(h.rt.state.focus, PanelId::Agents, "tab wraps");
    h.send(key(KeyCode::BackTab));
    assert_eq!(h.rt.state.focus, PanelId::Work, "shift-tab wraps back");
    h.key('3');
    assert_eq!(h.rt.state.focus, PanelId::Queue);
}

#[test]
fn j_k_and_the_arrows_move_the_selection_and_stay_inside_the_list() {
    let mut h = harness();
    h.up();
    h.key('1');
    assert!(h.text().contains("sel a=0"));
    h.key('j');
    h.send(key(KeyCode::Down));
    assert!(h.text().contains("sel a=2"));
    h.key('k');
    h.send(key(KeyCode::Up));
    h.send(key(KeyCode::Up));
    assert!(h.text().contains("sel a=0"), "clamped at the top");
    for _ in 0..100 {
        h.key('j');
    }
    let last = h.rows()[3].clone();
    let model = h.rt.state.model.as_ref().unwrap();
    let count = cstan_dash::model::visible_agents(model, false).len();
    assert!(last.contains(&format!("sel a={}", count - 1)), "{last}");
}

#[test]
fn f_filters_the_queue_and_e_shows_every_ended_agent_only_in_their_panels() {
    let mut h = harness();
    h.up();
    h.key('f');
    assert!(h.rt.state.problems_only);
    h.key('f');
    assert!(!h.rt.state.problems_only);
    h.key('e');
    assert!(
        !h.rt.state.show_all_ended,
        "e does nothing in the queue panel"
    );
    h.key('1');
    h.key('f');
    assert!(
        !h.rt.state.problems_only,
        "f does nothing outside the queue"
    );
    h.key('e');
    assert!(h.rt.state.show_all_ended);
    h.key('e');
    assert!(!h.rt.state.show_all_ended);
}

#[test]
fn p_pauses_r_polls_now_and_the_interval_keys_step_within_1_to_60() {
    let mut h = harness();
    h.up();
    h.key('p');
    assert!(h.text().contains("paused=true"));
    h.key('p');
    h.key('r');
    assert_eq!(
        h.recorder.log(),
        ["paused true", "paused false", "poll_now"]
    );
    // 2 s: down to 1, then no further.
    h.key('-');
    h.key('-');
    h.key('-');
    assert!(h.text().contains("every=1s"));
    h.key('+');
    h.key('=');
    assert!(h.text().contains("every=3s"));
    assert_eq!(
        h.recorder.log()[3..],
        ["interval 1000", "interval 2000", "interval 3000"]
    );
}

#[test]
fn help_opens_with_question_mark_and_any_key_closes_it() {
    let mut h = harness();
    h.up();
    h.key('?');
    assert!(h.text().contains("HELP overlay"));
    h.key('?');
    assert!(!h.text().contains("HELP overlay"));
    h.key('?');
    h.key('x');
    assert!(!h.text().contains("HELP overlay"));
    assert!(!h.rt.quit_requested());
}

#[test]
fn q_and_ctrl_c_quit() {
    let mut h = harness();
    h.up();
    h.key('q');
    assert_eq!(h.rt.exit, Some(0));
    let mut c = harness();
    c.up();
    c.send(Event::Input(TerminalEvent::Key(KeyEvent::new(
        KeyCode::Char('c'),
        KeyModifiers::CONTROL,
    ))));
    assert_eq!(c.rt.exit, Some(0));
    let mut s = harness();
    s.send(Event::Signal(15));
    assert_eq!(s.rt.exit, Some(143));
}

#[test]
fn key_releases_and_other_terminal_events_do_nothing() {
    let release = KeyEvent {
        kind: crossterm::event::KeyEventKind::Release,
        ..KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)
    };
    assert_eq!(keys::from_event(&release), None);
    assert_eq!(
        keys::from_event(&KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        Some(Key::CtrlC)
    );
    assert_eq!(
        keys::from_event(&KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT)),
        Some(Key::Other)
    );
    let mut h = harness();
    h.up();
    h.send(Event::Input(TerminalEvent::Resize(80, 24)));
    assert!(!h.rt.quit_requested());
}

// ---- actions: confirm, delay, calls, notices ---------------------------------------------------------------------

fn first_message(h: &Harness) -> cstan_dash::model::MessageRow {
    h.selected_message()
}

#[test]
fn an_action_key_opens_a_confirm_prompt_and_any_other_key_cancels_without_a_call() {
    let mut h = harness();
    h.up();
    let message = first_message(&h);
    h.key('s');
    let text = confirm_text(&DashAction::Resolve {
        decision: cstan_dash::model::Decision::Skip,
        message: message.clone(),
    });
    assert!(h.overlay_text().contains(&text), "{}", h.text());
    assert!(text.contains("Press y again to confirm, any other key cancels."));
    h.key('x');
    assert!(h.recorder.calls().is_empty());
    assert!(h.text().contains("cancelled"));
    assert!(!h.text().contains("Skip message"));
}

#[test]
fn y_then_y_after_the_confirm_delay_runs_the_call_once_and_a_held_key_does_not() {
    let mut h = harness();
    h.up();
    let message = first_message(&h);
    h.key('y');
    assert!(
        h.text()
            .contains("The recipient may already have received it.")
            == (message.state == "sent" || message.state == "unacked")
    );
    h.advance(100);
    h.key('y');
    assert!(
        h.recorder.calls().is_empty(),
        "a second y inside 300 ms does not confirm"
    );
    assert!(h.rt.state.prompt.is_some(), "and the prompt stays up");
    h.advance(200);
    h.key('y');
    assert_eq!(
        h.recorder.calls(),
        vec![DashAction::Resolve {
            decision: cstan_dash::model::Decision::Retry,
            message: message.clone()
        }]
    );
    assert!(
        h.text().contains("working..."),
        "busy shows while the call runs"
    );
    // Nothing else can act while the call is open.
    h.key('s');
    assert!(h.rt.state.prompt.is_none());
    h.send(Event::CallDone {
        action: h.recorder.calls()[0].clone(),
        result: CallResult::Ok(json!({"state": "queued"})),
    });
    assert!(h
        .text()
        .contains(&format!("retry {}: queued", message.message_id)));
    assert_eq!(
        h.recorder.log(),
        ["poll_now"],
        "a finished action polls at once"
    );
}

#[test]
fn calls_use_the_wire_names_and_arguments() {
    let mut h = harness_with(false, 0);
    h.up();
    let message = first_message(&h);
    for (key, command, args) in [
        ('c', "cancel", vec![message.message_id.clone()]),
        (
            's',
            "resolve",
            vec![message.message_id.clone(), "skip".to_string()],
        ),
    ] {
        h.key(key);
        h.key('y');
        let action = h.recorder.calls().last().unwrap().clone();
        assert_eq!(
            cstan_dash::model::to_wire_call(&action),
            (command.to_string(), args)
        );
        h.send(Event::CallDone {
            action,
            result: CallResult::Ok(json!({"state": "cancelled"})),
        });
    }
}

#[test]
fn a_refusal_from_the_daemon_is_shown_cleaned_and_cut() {
    let mut h = harness_with(false, 0);
    h.up();
    h.key('s');
    h.key('y');
    let action = h.recorder.calls()[0].clone();
    h.send(Event::CallDone {
        action,
        result: CallResult::Failed(format!(
            "illegal_resolution:\u{1b}[31m no {}",
            "x".repeat(400)
        )),
    });
    let notice = h.rt.state.notice.clone().unwrap();
    assert!(
        notice.starts_with("illegal_resolution: [31m no"),
        "{notice}"
    );
    assert!(!notice.contains('\u{1b}'));
    assert_eq!(notice.chars().count(), 200);
    assert!(notice.ends_with('…'));
}

#[test]
fn a_key_that_does_not_apply_to_the_message_state_says_so_and_opens_no_prompt() {
    let mut h = harness();
    let mut model = showcase();
    let messages = model["queue"]["messages"].as_array_mut().unwrap();
    messages.truncate(1);
    messages[0]["state"] = json!("queued");
    h.status(&model);
    h.key('y');
    assert!(h.rt.state.prompt.is_none());
    assert!(h
        .text()
        .contains("retry does not apply to a message in state queued"));
    assert!(h.recorder.calls().is_empty());
}

#[test]
fn actions_are_ignored_while_paused_or_down_and_on_an_empty_queue() {
    let mut h = harness();
    h.up();
    h.key('p');
    h.key('s');
    assert!(h.rt.state.prompt.is_none(), "paused");
    h.key('p');
    h.fail(Link::Down);
    h.key('s');
    assert!(h.rt.state.prompt.is_none(), "link down");
    let mut empty = harness();
    let mut model = showcase();
    model["queue"]["messages"] = json!([]);
    empty.status(&model);
    for k in ['y', 's', 'c'] {
        empty.key(k);
    }
    assert!(empty.rt.state.prompt.is_none());
    assert!(empty.recorder.calls().is_empty());
}

#[test]
fn the_other_panels_do_not_open_prompts() {
    let mut h = harness();
    h.up();
    h.key('1');
    for k in ['y', 's', 'c'] {
        h.key(k);
    }
    assert!(h.rt.state.prompt.is_none());
}

#[test]
fn a_change_to_the_message_under_an_open_prompt_cancels_it() {
    let mut h = harness();
    h.up();
    let message = first_message(&h);
    h.key('s');
    assert!(h.rt.state.prompt.is_some());
    let mut changed = showcase();
    changed["queue"]["messages"][0]["state"] = json!("acked_late");
    h.status(&changed);
    assert!(h.rt.state.prompt.is_none());
    assert!(h.text().contains("cancelled: the message changed"));
    h.key('y');
    assert!(h.recorder.calls().is_empty(), "{}", message.message_id);
    // The same holds when the message is gone.
    let mut again = harness();
    again.up();
    again.key('s');
    let mut gone = showcase();
    gone["queue"]["messages"] = json!([]);
    again.status(&gone);
    assert!(again.rt.state.prompt.is_none());
}

#[test]
fn o_on_an_active_agent_peeks_through_a_call_and_esc_or_q_closes_the_overlay() {
    let mut h = harness();
    h.up();
    h.key('1');
    let model = h.rt.state.model.as_ref().unwrap();
    let agent = cstan_dash::model::visible_agents(model, false)[0].clone();
    assert_eq!(agent.state, "active");
    h.key('o');
    let calls = h.recorder.calls();
    assert_eq!(
        calls,
        vec![DashAction::Observe {
            agent_id: agent.agent_id.clone()
        }]
    );
    assert_eq!(
        cstan_dash::model::to_wire_call(&calls[0]),
        (
            "peek".to_string(),
            vec![agent.agent_id.clone(), "40".to_string()]
        )
    );
    h.send(Event::CallDone {
        action: calls[0].clone(),
        result: CallResult::Ok(json!({"agentStatus": "idle", "text": "line one\nline two"})),
    });
    assert!(h
        .text()
        .contains(&format!("observe {} (idle)", agent.agent_id)));
    assert!(
        h.text().contains("sel a=0"),
        "the dashboard stays under the box"
    );
    // Keys other than Esc and q do nothing while the peek is up, and the dashboard keys stay dead.
    h.key('j');
    assert!(h.rt.state.peek.is_some());
    h.send(key(KeyCode::Esc));
    assert!(h.rt.state.peek.is_none());
    assert!(!h.text().contains("observe "));
    h.key('o');
    let call = h.recorder.calls()[1].clone();
    h.send(Event::CallDone {
        action: call,
        result: CallResult::Ok(json!({"text": 5})),
    });
    assert!(
        h.text().contains("(unknown)"),
        "a missing status reads unknown"
    );
    h.key('q');
    assert!(h.rt.state.peek.is_none());
    assert!(
        !h.rt.quit_requested(),
        "q closes the peek rather than quitting"
    );
}

#[test]
fn a_refused_peek_is_a_notice_and_an_ended_agent_cannot_be_observed() {
    let mut h = harness();
    h.up();
    h.key('1');
    h.key('o');
    let action = h.recorder.calls()[0].clone();
    h.send(Event::CallDone {
        action,
        result: CallResult::Failed("agent_not_active".to_string()),
    });
    assert!(h.rt.state.peek.is_none());
    assert!(h.text().contains("agent_not_active"));
    // Move to an ended agent.
    let model = h.rt.state.model.as_ref().unwrap();
    let rows = cstan_dash::model::visible_agents(model, false);
    let ended = rows
        .iter()
        .position(|a| a.state != "active")
        .expect("an ended agent");
    let agent_id = rows[ended].agent_id.clone();
    for _ in 0..ended {
        h.key('j');
    }
    h.key('o');
    assert_eq!(h.recorder.calls().len(), 1, "no second call");
    assert!(h.text().contains(&format!(
        "{agent_id} has ended; observe needs an active agent"
    )));
}

#[test]
fn the_bell_rings_once_for_a_new_signal_and_not_on_the_first_poll_or_under_reduced_motion() {
    let mut h = harness();
    h.up();
    assert_eq!(
        h.bells.load(Ordering::SeqCst),
        0,
        "the first poll never rings"
    );
    let mut stuck = showcase();
    stuck["stuck"] = json!([{"messageId": "m-new", "reason": "no_ack"}]);
    h.status(&stuck);
    assert_eq!(h.bells.load(Ordering::SeqCst), 1);
    let mut again = stuck.clone();
    again["counts"]["unresolved"] = json!(7);
    h.status(&again);
    assert_eq!(
        h.bells.load(Ordering::SeqCst),
        1,
        "the same signal does not ring twice"
    );
    let mut quiet = harness_with(true, 300);
    quiet.up();
    quiet.status(&stuck);
    assert_eq!(quiet.bells.load(Ordering::SeqCst), 0);
}

#[test]
fn signals_cover_notified_clears_stuck_lost_and_escalated() {
    let status = json!({
        "messages": [{"messageId": "a", "lastNotifiedAt": "t1"}, {"messageId": "b", "lastNotifiedAt": null}],
        "inputClears": [{"clearId": "c1"}],
        "stuck": [{"messageId": "s", "reason": "r"}],
        "lostAgentIds": ["x"],
        "agentFindings": [{"findingId": "f1", "state": "escalated"}, {"findingId": "f2", "state": "open"}],
    });
    let mut signals: Vec<String> = signals_of(&status).into_iter().collect();
    signals.sort();
    assert_eq!(
        signals,
        [
            "clear:c1",
            "finding-escalated:f1",
            "lost:x",
            "notified:a:t1",
            "stuck:s:r"
        ]
    );
    assert!(signals_of(&json!({})).is_empty());
}

#[test]
fn the_rings_are_capped_at_300_and_a_changed_row_is_highlighted_for_two_polls() {
    let mut h = harness();
    h.up();
    for _ in 0..(RING_LIMIT + 20) {
        h.unchanged(&showcase());
    }
    assert_eq!(h.rt.state.rings.working.len(), RING_LIMIT);
    assert_eq!(h.rt.state.rings.unresolved.len(), RING_LIMIT);
    assert_eq!(h.rt.state.rings.oldest.len(), RING_LIMIT);
    let view = |h: &Harness| {
        h.rt.state
            .view_state(
                Size {
                    columns: COLUMNS,
                    rows: ROWS,
                },
                h.now,
                false,
            )
            .highlight
    };
    assert!(view(&h).is_empty(), "the first poll highlights nothing");
    let mut changed = showcase();
    changed["agents"][0]["fingerprint"] = json!("changed-print");
    h.status(&changed);
    assert_eq!(view(&h).len(), 1);
    assert_eq!(view(&h)[0].1, 2);
    let mut next = changed.clone();
    next["counts"]["unresolved"] = json!(8);
    h.status(&next);
    assert_eq!(view(&h)[0].1, 1);
    let mut last = next.clone();
    last["counts"]["unresolved"] = json!(9);
    h.status(&last);
    assert!(view(&h).is_empty());
}

#[test]
fn a_selection_follows_its_row_id_and_falls_back_to_the_old_position() {
    let ids: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
    let at = |id: Option<&str>, index| Selection {
        id: id.map(str::to_string),
        index,
    };
    assert_eq!(resolve_selection(&ids, &at(Some("c"), 0)), 2);
    assert_eq!(resolve_selection(&ids, &at(Some("gone"), 1)), 1);
    assert_eq!(resolve_selection(&ids, &at(Some("gone"), 9)), 2);
    assert_eq!(resolve_selection(&[], &at(Some("a"), 3)), 0);
    assert_eq!(resolve_selection(&ids, &at(None, 1)), 1);
}

#[test]
fn row_ids_follow_the_problems_filter() {
    let model: DashModel = serde_json::from_value(showcase()).unwrap();
    let all = row_ids(&model, false, false);
    let problems = row_ids(&model, true, false);
    assert_eq!(all[2].len(), model.queue.messages.len());
    assert!(problems[2].len() < all[2].len());
    assert!(row_ids(&model, false, true)[0].len() >= all[0].len());
}

#[test]
fn the_clock_is_zero_padded_hours_minutes_and_seconds() {
    let clock = cstan_dash::app::state::format_clock(NOW);
    let parts: Vec<&str> = clock.split(':').collect();
    assert_eq!(parts.len(), 3);
    assert!(parts
        .iter()
        .all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_digit())));
    assert_eq!(cstan_dash::app::state::age_since(NOW - 90_000, NOW), "1m");
    assert_eq!(cstan_dash::app::state::age_since(NOW, NOW - 5_000), "0s");
}

// ---- painting ----------------------------------------------------------------------------------------------------

#[test]
fn a_wide_grapheme_takes_two_cells_and_a_combining_mark_joins_its_base() {
    let area = Rect::new(0, 0, 10, 1);
    let mut buffer = Buffer::empty(area);
    paint_line(
        &mut buffer,
        area,
        0,
        0,
        &plain("a\u{4e2d}e\u{301}b"),
        ColorDepth::TrueColor,
        width_of,
    );
    assert_eq!(buffer[(0, 0)].symbol(), "a");
    assert_eq!(buffer[(1, 0)].symbol(), "\u{4e2d}");
    assert_eq!(buffer[(2, 0)].symbol(), "");
    assert_eq!(buffer[(3, 0)].symbol(), "e\u{301}");
    assert_eq!(buffer[(4, 0)].symbol(), "b");
}

#[test]
fn a_wide_character_that_does_not_fit_is_dropped_and_an_overlay_clears_half_a_wide_character() {
    let area = Rect::new(0, 0, 4, 2);
    let mut buffer = Buffer::empty(area);
    paint_line(
        &mut buffer,
        area,
        0,
        0,
        &plain("abc\u{4e2d}"),
        ColorDepth::TrueColor,
        width_of,
    );
    assert_eq!(
        buffer[(3, 0)].symbol(),
        " ",
        "no half character at the edge"
    );
    let mut over = Buffer::empty(area);
    paint_line(
        &mut over,
        area,
        0,
        0,
        &plain("\u{4e2d}\u{4e2d}"),
        ColorDepth::TrueColor,
        width_of,
    );
    paint_line(
        &mut over,
        area,
        1,
        0,
        &plain("X"),
        ColorDepth::TrueColor,
        width_of,
    );
    assert_eq!(over[(0, 0)].symbol(), " ", "the left half is blanked");
    assert_eq!(over[(1, 0)].symbol(), "X");
    assert_eq!(over[(2, 0)].symbol(), "\u{4e2d}");
}

#[test]
fn styles_map_to_cells_and_colours_downsample_by_depth() {
    let area = Rect::new(0, 0, 4, 1);
    let mut buffer = Buffer::empty(area);
    let line = vec![Span {
        text: "x".to_string(),
        color: Some("#77ca9b".to_string()),
        bg: Some("#3b4252".to_string()),
        bold: Some(true),
        dim: Some(true),
    }];
    paint_line(
        &mut buffer,
        area,
        0,
        0,
        &line,
        ColorDepth::TrueColor,
        width_of,
    );
    let cell = &buffer[(0, 0)];
    assert_eq!(cell.fg, Color::Rgb(0x77, 0xca, 0x9b));
    assert_eq!(cell.bg, Color::Rgb(0x3b, 0x42, 0x52));
    assert!(cell.modifier.contains(Modifier::BOLD | Modifier::DIM));
    assert_eq!(
        color_of("#ff0000", ColorDepth::Ansi256),
        Some(Color::Indexed(196))
    );
    assert_eq!(
        color_of("#808080", ColorDepth::Ansi256),
        Some(Color::Indexed(244))
    );
    for (hex, want) in [
        ("#ff0000", Color::LightRed),
        ("#cd0000", Color::Red),
        ("#00ff00", Color::LightGreen),
        ("#101010", Color::Black),
        ("#6c6c6c", Color::DarkGray),
    ] {
        assert_eq!(color_of(hex, ColorDepth::Ansi16), Some(want), "{hex}");
    }
    assert_eq!(color_of("not a colour", ColorDepth::TrueColor), None);
    assert_eq!(
        color_of("#fff", ColorDepth::TrueColor),
        Some(Color::Rgb(255, 255, 255))
    );
    let plain_cell = {
        let mut b = Buffer::empty(area);
        paint_line(
            &mut b,
            area,
            0,
            0,
            &plain("y"),
            ColorDepth::TrueColor,
            width_of,
        );
        b[(0, 0)].clone()
    };
    assert_eq!(
        plain_cell.fg,
        Color::Reset,
        "an absent colour keeps the terminal default"
    );
}

#[test]
fn colour_depth_follows_colorterm_and_term() {
    assert_eq!(
        ColorDepth::detect(Some("truecolor"), Some("xterm")),
        ColorDepth::TrueColor
    );
    assert_eq!(
        ColorDepth::detect(Some("24bit"), None),
        ColorDepth::TrueColor
    );
    assert_eq!(
        ColorDepth::detect(None, Some("xterm-256color")),
        ColorDepth::Ansi256
    );
    assert_eq!(
        ColorDepth::detect(Some(""), Some("xterm")),
        ColorDepth::Ansi16
    );
    assert_eq!(ColorDepth::detect(None, None), ColorDepth::Ansi16);
}

#[test]
fn an_overlay_is_painted_over_the_lines_and_clipped_to_the_screen() {
    let area = Rect::new(0, 0, 12, 3);
    let mut buffer = Buffer::empty(area);
    let lines = vec![
        plain("aaaaaaaaaaaa"),
        plain("bbbbbbbbbbbb"),
        plain("cccccccccccc"),
    ];
    let overlay = Overlay {
        lines: vec![plain("[XXXXXXXX]"), plain("[YYYYYYYY]"), plain("[ZZZZ]")],
        top: 1,
        left: 6,
    };
    paint_screen(
        &mut buffer,
        area,
        &lines,
        Some(&overlay),
        ColorDepth::TrueColor,
        width_of,
    );
    assert_eq!(
        screen(&buffer),
        ["aaaaaaaaaaaa", "bbbbbb[XXXXX", "cccccc[YYYYY"]
    );
}

// ---- the poller --------------------------------------------------------------------------------------------------

#[test]
fn the_poller_tables_match_the_node_implementation() {
    let table = common::load_fixture("poller");
    assert_eq!(table["backoffMs"], json!(BACKOFF_MS));
    for row in table["nextDelayMs"].as_array().unwrap() {
        let out = next_delay_ms(
            row["failures"].as_u64().unwrap() as u32,
            row["intervalMs"].as_u64().unwrap(),
            &BACKOFF_MS,
        );
        assert_eq!(json!(out), row["out"], "{row}");
    }
    for row in table["stepInterval"].as_array().unwrap() {
        let out = step_interval(
            row["seconds"].as_u64().unwrap() as u32,
            row["faster"].as_bool().unwrap(),
        );
        assert_eq!(json!(out), row["out"], "{row}");
    }
}

#[test]
fn the_status_hash_is_stable_and_changes_with_the_content() {
    assert_eq!(
        status_hash(&json!({"a": 1, "b": [1, 2]})),
        status_hash(&json!({"b": [1, 2], "a": 1}))
    );
    assert_ne!(status_hash(&json!({"a": 1})), status_hash(&json!({"a": 2})));
}

fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if done() {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("timed out waiting for {what}");
}

type Script = Arc<Mutex<Vec<Result<Value, ClientError>>>>;

fn scripted_poller(
    interval_ms: u64,
    backoff: Vec<u64>,
    script: Script,
) -> (
    Poller,
    mpsc::Receiver<PollEvent>,
    Arc<AtomicUsize>,
    Arc<AtomicUsize>,
) {
    let (sender, receiver) = mpsc::channel();
    let running = Arc::new(AtomicUsize::new(0));
    let overlaps = Arc::new(AtomicUsize::new(0));
    let (r, o) = (Arc::clone(&running), Arc::clone(&overlaps));
    let poller = Poller::spawn(
        interval_ms,
        backoff,
        move || {
            if r.fetch_add(1, Ordering::SeqCst) != 0 {
                o.fetch_add(1, Ordering::SeqCst);
            }
            std::thread::sleep(Duration::from_millis(3));
            r.fetch_sub(1, Ordering::SeqCst);
            let mut script = script.lock().unwrap();
            if script.is_empty() {
                Ok(json!({"same": true}))
            } else {
                script.remove(0)
            }
        },
        move |event| {
            let _ = sender.send(event);
        },
    );
    (poller, receiver, running, overlaps)
}

#[test]
fn an_unchanged_status_is_reported_as_unchanged_and_requests_never_overlap() {
    let script: Script = Arc::new(Mutex::new(vec![
        Ok(json!({"n": 1})),
        Ok(json!({"n": 1})),
        Ok(json!({"n": 2})),
    ]));
    let (poller, events, _, overlaps) = scripted_poller(5, vec![5, 5, 5], script);
    let flags: Vec<bool> = (0..3)
        .map(
            |_| match events.recv_timeout(Duration::from_secs(5)).unwrap() {
                PollEvent::Status { changed, .. } => changed,
                other => panic!("{other:?}"),
            },
        )
        .collect();
    assert_eq!(flags, [true, false, true]);
    poller.join();
    assert_eq!(overlaps.load(Ordering::SeqCst), 0);
}

#[test]
fn failures_back_off_and_a_later_success_recovers_the_interval() {
    let script: Script = Arc::new(Mutex::new(vec![
        Err(ClientError::Unreachable),
        Err(ClientError::TooLarge),
        Err(ClientError::Unreachable),
        Err(ClientError::Unreachable),
    ]));
    // The scaled table: 20 ms, 40 ms, 80 ms against a 5 ms interval.
    let (poller, events, _, _) = scripted_poller(5, vec![20, 40, 80], script);
    let mut stamps = Vec::new();
    let mut links = Vec::new();
    for _ in 0..6 {
        let event = events.recv_timeout(Duration::from_secs(5)).unwrap();
        stamps.push(Instant::now());
        links.push(match event {
            PollEvent::Failed { link } => Some(link),
            PollEvent::Status { .. } => None,
        });
    }
    assert_eq!(
        links,
        [
            Some(Link::Down),
            Some(Link::Toolarge),
            Some(Link::Down),
            Some(Link::Down),
            None,
            None
        ]
    );
    let gap = |i: usize| stamps[i + 1].duration_since(stamps[i]);
    assert!(
        gap(0) >= Duration::from_millis(19),
        "after one failure: 20 ms"
    );
    assert!(gap(1) >= Duration::from_millis(39), "after two: 40 ms");
    assert!(gap(2) >= Duration::from_millis(79), "after three: 80 ms");
    assert!(
        gap(3) >= Duration::from_millis(79),
        "stays at the last step"
    );
    assert!(
        gap(4) < Duration::from_millis(60),
        "a success returns to the interval"
    );
    poller.join();
}

#[test]
fn pause_stops_polling_a_forced_poll_still_runs_and_stop_ends_the_loop() {
    let (poller, events, _, _) = scripted_poller(10, vec![10], Arc::new(Mutex::new(Vec::new())));
    events.recv_timeout(Duration::from_secs(5)).unwrap();
    poller.set_paused(true);
    std::thread::sleep(Duration::from_millis(40));
    while events.try_recv().is_ok() {}
    std::thread::sleep(Duration::from_millis(60));
    assert!(events.try_recv().is_err(), "paused: no polls");
    poller.poll_now();
    events
        .recv_timeout(Duration::from_secs(5))
        .expect("a forced poll runs while paused");
    std::thread::sleep(Duration::from_millis(60));
    assert!(events.try_recv().is_err(), "and only one");
    poller.set_paused(false);
    events
        .recv_timeout(Duration::from_secs(5))
        .expect("polling resumes");
    poller.join();
    while events.try_recv().is_ok() {}
    std::thread::sleep(Duration::from_millis(40));
    assert!(events.try_recv().is_err(), "stopped");
}

#[test]
fn a_new_interval_restarts_the_sleep_and_does_not_poll_early() {
    let (poller, events, _, _) =
        scripted_poller(60_000, vec![60_000], Arc::new(Mutex::new(Vec::new())));
    events.recv_timeout(Duration::from_secs(5)).unwrap();
    std::thread::sleep(Duration::from_millis(30));
    let before = Instant::now();
    poller.set_interval_ms(80);
    assert!(
        events.try_recv().is_err(),
        "changing the interval does not poll by itself"
    );
    events.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(
        before.elapsed() >= Duration::from_millis(79),
        "a full new interval first"
    );
    poller.join();
}

#[test]
fn a_forced_poll_during_a_request_polls_again_without_waiting_out_the_interval() {
    let (poller, events, running, _) =
        scripted_poller(60_000, vec![60_000], Arc::new(Mutex::new(Vec::new())));
    events.recv_timeout(Duration::from_secs(5)).unwrap();
    wait_for("idle", || running.load(Ordering::SeqCst) == 0);
    poller.poll_now();
    poller.set_interval_ms(60_000);
    events
        .recv_timeout(Duration::from_secs(5))
        .expect("a forced poll still polls once");
    poller.join();
}

// ---- the socket client against a fake daemon -------------------------------------------------------------------------

struct FakeDaemon {
    path: PathBuf,
    requests: Arc<Mutex<Vec<Value>>>,
    _dir: tempfile::TempDir,
}

enum Reply {
    Json(Value),
    Raw(Vec<u8>),
    Silent,
    Close,
}

fn daemon(handler: impl Fn(&Value) -> Reply + Send + 'static) -> FakeDaemon {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("d.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&requests);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut line = String::new();
            if BufReader::new(&stream).read_line(&mut line).is_err() {
                continue;
            }
            let Ok(request) = serde_json::from_str::<Value>(line.trim_end()) else {
                continue;
            };
            log.lock().unwrap().push(request.clone());
            match handler(&request) {
                Reply::Json(value) => {
                    let _ = writeln!(stream, "{value}");
                }
                Reply::Raw(bytes) => {
                    let _ = stream.write_all(&bytes);
                }
                Reply::Silent => std::thread::sleep(Duration::from_millis(800)),
                Reply::Close => {}
            }
        }
    });
    FakeDaemon {
        path,
        requests,
        _dir: dir,
    }
}

fn client_of(daemon: &FakeDaemon) -> Client {
    Client::new(daemon.path.clone(), "secret".to_string())
}

fn ok(result: Value) -> Reply {
    Reply::Json(json!({"ok": true, "result": result}))
}

#[test]
fn a_call_sends_the_versioned_frame_and_status_returns_the_result() {
    let d = daemon(|_| ok(json!({"hello": 1})));
    let client = client_of(&d);
    assert_eq!(client.status().unwrap(), json!({"hello": 1}));
    assert_eq!(
        d.requests.lock().unwrap()[0],
        json!({"v": 1, "credential": "secret", "command": "status", "args": []})
    );
}

#[test]
fn actions_go_over_the_wire_with_the_same_commands_and_arguments() {
    let d = daemon(|request| ok(json!({"state": "queued", "echo": request["args"]})));
    let client = client_of(&d);
    let model: DashModel = serde_json::from_value(showcase()).unwrap();
    let message = model.queue.messages[0].clone();
    for (decision, command, args) in [
        (
            cstan_dash::model::Decision::Retry,
            "resolve",
            json!([message.message_id, "retry"]),
        ),
        (
            cstan_dash::model::Decision::Skip,
            "resolve",
            json!([message.message_id, "skip"]),
        ),
        (
            cstan_dash::model::Decision::Cancel,
            "cancel",
            json!([message.message_id]),
        ),
    ] {
        let action = DashAction::Resolve {
            decision,
            message: message.clone(),
        };
        assert!(matches!(client.run_action(&action), CallResult::Ok(_)));
        let last = d.requests.lock().unwrap().last().unwrap().clone();
        assert_eq!(last["command"], command);
        assert_eq!(last["args"], args);
    }
    client.run_action(&DashAction::Observe {
        agent_id: "developer-2".to_string(),
    });
    let last = d.requests.lock().unwrap().last().unwrap().clone();
    assert_eq!(
        (last["command"].clone(), last["args"].clone()),
        (json!("peek"), json!(["developer-2", "40"]))
    );
}

#[test]
fn a_refusal_carries_its_message_and_status_treats_it_as_unreachable() {
    let d = daemon(|_| {
        Reply::Json(
            json!({"ok": false, "code": "illegal_resolution", "message": "illegal_resolution: no"}),
        )
    });
    let client = client_of(&d);
    assert_eq!(
        client
            .call("resolve", &["m".to_string(), "retry".to_string()])
            .unwrap(),
        Response::Refused {
            code: "illegal_resolution".to_string(),
            message: "illegal_resolution: no".to_string()
        }
    );
    assert_eq!(
        client.call_text("cancel", &[]),
        CallResult::Failed("illegal_resolution: no".to_string())
    );
    assert_eq!(client.status(), Err(ClientError::Unreachable));
}

#[test]
fn malformed_replies_closed_connections_and_missing_sockets_are_unreachable() {
    for bytes in [
        b"not json\n".to_vec(),
        b"[1]\n".to_vec(),
        b"{\"result\":1}\n".to_vec(),
        b"\xff\xfe\n".to_vec(),
        b"{\"ok\":true".to_vec(),
    ] {
        let d = daemon(move |_| Reply::Raw(bytes.clone()));
        assert_eq!(client_of(&d).status(), Err(ClientError::Unreachable));
    }
    let closed = daemon(|_| Reply::Close);
    assert_eq!(client_of(&closed).status(), Err(ClientError::Unreachable));
    let gone = Client::new(
        Path::new("/nonexistent/none.sock").to_path_buf(),
        "x".to_string(),
    );
    assert_eq!(gone.status(), Err(ClientError::Unreachable));
    assert_eq!(
        gone.call_text("peek", &[]),
        CallResult::Failed("the controller did not answer".to_string())
    );
}

#[test]
fn a_daemon_that_does_not_answer_times_out() {
    let d = daemon(|_| Reply::Silent);
    let mut client = client_of(&d);
    client.timeout = Duration::from_millis(150);
    let started = Instant::now();
    assert_eq!(client.status(), Err(ClientError::Unreachable));
    assert!(started.elapsed() < Duration::from_millis(700));
}

#[test]
fn a_response_line_over_one_mebibyte_is_too_large_and_one_at_the_limit_is_read() {
    let big = daemon(|_| {
        let mut bytes = vec![b'x'; MAX_RESPONSE_BYTES + 10];
        bytes.push(b'\n');
        Reply::Raw(bytes)
    });
    let error = client_of(&big).status().unwrap_err();
    assert_eq!(error, ClientError::TooLarge);
    assert_eq!(error.link(), Link::Toolarge);
    let unterminated = daemon(|_| Reply::Raw(vec![b'y'; MAX_RESPONSE_BYTES + 100]));
    assert_eq!(
        client_of(&unterminated).status(),
        Err(ClientError::TooLarge)
    );
    let body = {
        let prefix = "{\"ok\":true,\"result\":{\"pad\":\"";
        let suffix = "\"}}";
        let fill = MAX_RESPONSE_BYTES - prefix.len() - suffix.len();
        format!("{prefix}{}{suffix}", "z".repeat(fill))
    };
    assert_eq!(body.len(), MAX_RESPONSE_BYTES);
    let exact = daemon(move |_| Reply::Raw(format!("{body}\n").into_bytes()));
    assert!(client_of(&exact).status().is_ok());
}

#[test]
fn a_request_over_64_kib_is_refused_before_it_is_sent() {
    let d = daemon(|_| ok(json!({})));
    let client = client_of(&d);
    let args = vec!["a".repeat(MAX_FRAME_BYTES)];
    assert_eq!(
        client.call("resolve", &args),
        Err(ClientError::RequestTooLarge)
    );
    assert!(d.requests.lock().unwrap().is_empty());
    assert_eq!(ClientError::RequestTooLarge.link(), Link::Down);
}

// ---- the whole loop: poller thread, fake daemon, keys -----------------------------------------------------------

fn send_key(events: &Sender<Event>, c: char) {
    events.send(ch(c)).unwrap();
}

#[test]
fn the_live_loop_polls_the_daemon_confirms_an_action_and_shows_the_result() {
    let state_counter = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&state_counter);
    let model = showcase();
    let d = daemon(move |request| match request["command"].as_str().unwrap() {
        "status" => {
            counter.fetch_add(1, Ordering::SeqCst);
            ok(model.clone())
        }
        "resolve" | "cancel" => ok(json!({"state": "queued"})),
        _ => Reply::Json(json!({"ok": false, "code": "x", "message": "unknown"})),
    });
    let (sender, receiver) = mpsc::channel::<Event>();
    let client = Arc::new(client_of(&d));
    let controls = LiveControls::start(client, 50, vec![50], sender.clone());
    let mut state = AppState::new(fake_hooks(), 1, None, true);
    state.confirm_delay_ms = 100;
    let terminal = Terminal::new(TestBackend::new(COLUMNS, ROWS)).unwrap();
    let mut rt = Runtime::new(
        terminal,
        state,
        Theme::default(),
        fake_hooks(),
        ColorDepth::TrueColor,
        Box::new(controls),
        Box::new(|| {}),
    );
    let log = Arc::clone(&d.requests);
    let driver = std::thread::spawn(move || {
        wait_for("the first status", || !log.lock().unwrap().is_empty());
        std::thread::sleep(Duration::from_millis(100));
        send_key(&sender, '3');
        send_key(&sender, 'y');
        std::thread::sleep(Duration::from_millis(250));
        send_key(&sender, 'y');
        wait_for("the resolve call", || {
            log.lock()
                .unwrap()
                .iter()
                .any(|r| r["command"] == "resolve")
        });
        std::thread::sleep(Duration::from_millis(300));
        send_key(&sender, 'q');
    });
    rt.run_loop(&receiver, &|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
    })
    .unwrap();
    driver.join().unwrap();
    assert_eq!(rt.exit, Some(0));
    let text = screen(rt.terminal.backend().buffer()).join("\n");
    assert!(text.contains("link=Ok"), "{text}");
    assert!(text.contains(": queued"), "{text}");
    let requests = d.requests.lock().unwrap();
    let resolve: Vec<&Value> = requests
        .iter()
        .filter(|r| r["command"] == "resolve")
        .collect();
    assert_eq!(resolve.len(), 1);
    assert_eq!(resolve[0]["args"][1], "retry");
    assert!(
        requests.iter().filter(|r| r["command"] == "status").count() >= 2,
        "the action forces another poll"
    );
}

// ---- the command line and the terminal ---------------------------------------------------------------------------

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn usage_error(list: &[&str]) -> ArgError {
    parse_args(&args(list)).unwrap_err()
}

#[test]
fn the_flags_parse_in_any_order() {
    let options = parse_args(&args(&[
        "--reduced-motion",
        "--interval",
        "7",
        "--no-color",
        "--worker-limit",
        "3",
        "--socket",
        "/run/c.sock",
    ]))
    .unwrap();
    assert_eq!(options.socket, PathBuf::from("/run/c.sock"));
    assert_eq!(options.worker_limit, Some(3));
    assert_eq!(options.interval_seconds, 7);
    assert!(options.no_color && options.reduced_motion);
    let defaults = parse_args(&args(&["--socket", "/s"])).unwrap();
    assert_eq!(
        (
            defaults.interval_seconds,
            defaults.worker_limit,
            defaults.no_color
        ),
        (2, None, false)
    );
}

#[test]
fn bad_flags_exit_2_with_the_usage_line() {
    for bad in [
        vec![],
        vec!["--socket"],
        vec!["--socket", "relative.sock"],
        vec!["--socket", "/s", "--bogus"],
        vec!["--socket", "/s", "extra"],
        vec!["--socket", "/s", "--worker-limit"],
        vec!["--socket", "/s", "--worker-limit", "x"],
        vec!["--socket", "/s", "--worker-limit", "-1"],
        vec!["--socket", "/s", "--interval"],
    ] {
        let error = usage_error(&bad);
        assert_eq!(error.code, EXIT_USAGE, "{bad:?}");
        let text = error.render();
        assert!(text.starts_with("cstan-dash: "), "{text}");
        assert!(text.ends_with(&format!("{USAGE}\n")), "{text}");
        assert_eq!(text.lines().count(), 2);
    }
    assert_eq!(
        USAGE,
        "usage: cstan-dash --socket <path> [--worker-limit <n>] [--interval <seconds>] [--no-color] [--reduced-motion]"
    );
}

#[test]
fn a_bad_interval_exits_3_without_the_usage_line() {
    for bad in ["0", "61", "100", "-1", "x", "02", "1.5", ""] {
        let error = usage_error(&["--socket", "/s", "--interval", bad]);
        assert_eq!(error.code, EXIT_INTERVAL, "{bad:?}");
        assert_eq!(
            error.render(),
            "cstan-dash: --interval must be an integer from 1 to 60\n"
        );
    }
    for good in ["1", "9", "10", "60"] {
        assert!(
            parse_args(&args(&["--socket", "/s", "--interval", good])).is_ok(),
            "{good}"
        );
    }
}

#[test]
fn a_missing_or_empty_credential_exits_2_with_the_hint() {
    for value in [None, Some(String::new())] {
        let error = credential_from(value).unwrap_err();
        assert_eq!(error.code, EXIT_USAGE);
        assert_eq!(error.message, NO_CREDENTIAL_MESSAGE);
        assert_eq!(
            error.render(),
            "cstan-dash: no operator credential; start it with cstan dash\n"
        );
    }
    assert_eq!(credential_from(Some("tok".to_string())).unwrap(), "tok");
}

fn run_binary(arguments: &[&str], credential: Option<&str>) -> std::process::Output {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_cstan-dash"));
    command
        .args(arguments)
        .env_remove("CSTAN_DASH_CREDENTIAL")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if let Some(credential) = credential {
        command.env("CSTAN_DASH_CREDENTIAL", credential);
    }
    command.output().unwrap()
}

#[test]
fn the_binary_exits_2_for_usage_and_a_missing_credential_and_1_outside_a_terminal() {
    let missing = run_binary(&["--socket", "/tmp/none.sock"], None);
    assert_eq!(missing.status.code(), Some(2));
    assert_eq!(
        String::from_utf8_lossy(&missing.stderr),
        "cstan-dash: no operator credential; start it with cstan dash\n"
    );
    assert!(missing.stdout.is_empty());
    let bad = run_binary(&["--nope"], Some("t"));
    assert_eq!(bad.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&bad.stderr).contains("usage: cstan-dash --socket"));
    let interval = run_binary(
        &["--socket", "/tmp/none.sock", "--interval", "0"],
        Some("t"),
    );
    assert_eq!(interval.status.code(), Some(3));
    let piped = run_binary(&["--socket", "/tmp/none.sock"], Some("t"));
    assert_eq!(piped.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&piped.stderr).contains("dash needs an interactive terminal"));
    let version = run_binary(&["--version"], None);
    assert_eq!(version.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&version.stdout).starts_with("cstan-dash "));
}

#[test]
fn colour_glyph_and_motion_rules_follow_the_environment() {
    let env = |pairs: &'static [(&'static str, &'static str)]| {
        move |name: &str| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_string())
        }
    };
    assert!(is_utf8_locale(&env(&[])));
    assert!(is_utf8_locale(&env(&[("LANG", "en_US.UTF-8")])));
    assert!(is_utf8_locale(&env(&[("LANG", "C.utf8")])));
    assert!(!is_utf8_locale(&env(&[("LANG", "C")])));
    assert!(
        !is_utf8_locale(&env(&[("LC_ALL", "POSIX"), ("LANG", "en_US.UTF-8")])),
        "LC_ALL decides first"
    );
    assert!(
        is_utf8_locale(&env(&[("LC_ALL", ""), ("LANG", "en_US.UTF-8")])),
        "an empty value is skipped"
    );
    assert!(wants_ascii(&env(&[("TERM", "dumb")])));
    assert!(wants_ascii(&env(&[("LANG", "C")])));
    assert!(!wants_ascii(&env(&[("TERM", "xterm")])));
    assert!(wants_no_color(true, &env(&[])));
    assert!(wants_no_color(false, &env(&[("NO_COLOR", "1")])));
    assert!(!wants_no_color(false, &env(&[("NO_COLOR", "")])));
    assert!(wants_no_color(false, &env(&[("TERM", "dumb")])));
    assert!(!wants_no_color(false, &env(&[("TERM", "xterm")])));
    assert!(wants_reduced_motion(
        false,
        &env(&[("CSTAN_REDUCED_MOTION", "1")])
    ));
    assert!(!wants_reduced_motion(
        false,
        &env(&[("CSTAN_REDUCED_MOTION", "0")])
    ));
    assert!(wants_reduced_motion(true, &env(&[])));
}

#[derive(Clone, Default)]
struct MockOps {
    events: Arc<Mutex<Vec<&'static str>>>,
    fail: bool,
}

impl TerminalOps for MockOps {
    fn enter(&mut self) -> std::io::Result<()> {
        self.events.lock().unwrap().push("enter");
        if self.fail {
            Err(std::io::Error::other("no terminal"))
        } else {
            Ok(())
        }
    }
    fn leave(&mut self) {
        self.events.lock().unwrap().push("leave");
    }
}

#[test]
fn the_terminal_is_restored_once_on_every_way_out() {
    // Normal end.
    let ops = MockOps::default();
    {
        let _session = Session::enter(ops.clone()).unwrap();
    }
    assert_eq!(*ops.events.lock().unwrap(), ["enter", "leave"]);
    // Explicit leave, then drop: still once.
    let ops = MockOps::default();
    let mut session = Session::enter(ops.clone()).unwrap();
    session.leave();
    drop(session);
    assert_eq!(*ops.events.lock().unwrap(), ["enter", "leave"]);
    // A panic while the session is held.
    let ops = MockOps::default();
    let held = ops.clone();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _session = Session::enter(held).unwrap();
        panic!("the loop blew up");
    }));
    assert!(outcome.is_err());
    assert_eq!(
        *ops.events.lock().unwrap(),
        ["enter", "leave"],
        "restored while unwinding"
    );
    // Setup that fails never claims to have entered.
    let ops = MockOps {
        fail: true,
        ..MockOps::default()
    };
    assert!(Session::enter(ops.clone()).is_err());
    assert_eq!(*ops.events.lock().unwrap(), ["enter"]);
}

#[test]
fn a_panic_in_the_event_loop_unwinds_through_the_session_guard() {
    let ops = MockOps::default();
    let held = ops.clone();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _session = Session::enter(held).unwrap();
        let mut h = harness();
        h.up();
        // A hook that panics on build_frame models a bug in the view layer.
        let mut state = AppState::new(
            Hooks {
                build_frame: |_, _, _| panic!("view bug"),
                ..fake_hooks()
            },
            2,
            None,
            true,
        );
        state.on_poll(
            PollEvent::Status {
                status: showcase(),
                changed: true,
            },
            NOW,
        );
        let terminal = Terminal::new(TestBackend::new(COLUMNS, ROWS)).unwrap();
        let mut rt = Runtime::new(
            terminal,
            state,
            Theme::default(),
            Hooks {
                build_frame: |_, _, _| panic!("view bug"),
                ..fake_hooks()
            },
            ColorDepth::TrueColor,
            Box::new(cstan_dash::app::NoControls),
            Box::new(|| {}),
        );
        let _ = rt.redraw_if_needed(NOW);
    }));
    assert!(outcome.is_err());
    assert_eq!(*ops.events.lock().unwrap(), ["enter", "leave"]);
}

// ---- the real view layer (the model builder is still faked until dash-model lands) ---------------------------------

#[test]
fn the_real_view_draws_the_dashboard_and_its_overlays() {
    let hooks = Hooks {
        build_model: fake_model,
        ..Hooks::default()
    };
    let recorder = Recorder::default();
    let state = AppState::new(hooks, 2, Some(3), false);
    let terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let theme = cstan_dash::view::make_theme(cstan_dash::view::ThemeOptions::default());
    let mut rt = Runtime::new(
        terminal,
        state,
        theme,
        hooks,
        ColorDepth::TrueColor,
        Box::new(FakeControls(recorder)),
        Box::new(|| {}),
    );
    let text = |rt: &Runtime<TestBackend>| screen(rt.terminal.backend().buffer()).join("\n");
    rt.handle(
        Event::Poll(PollEvent::Status {
            status: showcase(),
            changed: true,
        }),
        NOW,
    );
    rt.on_time(NOW);
    assert!(rt.redraw_if_needed(NOW).unwrap());
    let base = text(&rt);
    assert!(base.contains("developer-2"), "{base}");
    assert!(
        !rt.state.shown.is_empty(),
        "the frame reports the panels shown"
    );
    rt.handle(ch('s'), NOW);
    rt.redraw_if_needed(NOW).unwrap();
    assert!(text(&rt).contains("Skip message"), "{}", text(&rt));
    rt.handle(ch('x'), NOW);
    rt.handle(ch('?'), NOW);
    rt.redraw_if_needed(NOW).unwrap();
    assert_ne!(text(&rt), base, "help is drawn over the dashboard");
    rt.handle(ch('?'), NOW);
    rt.redraw_if_needed(NOW).unwrap();
    assert!(text(&rt).contains("developer-2"));
}
