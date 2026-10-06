//! What the operator has done and seen: selection, filters, prompts, notices, history. No terminal and no sockets here.
use std::collections::HashSet;

use serde_json::Value;

use super::client::CallResult;
use super::keys::Key;
use super::poller::{step_interval, PollEvent};
use crate::model::text::clean;
use crate::model::{
    available_decisions, history_sample, observe_action, pipeline_items, queue_rows, track_changes,
    visible_agents, ChangeState, DashAction, DashModel, Decision,
};
use crate::view::{
    Frame, Link, Overlay, PanelId, PeekView, Rings, Selected, Size, Theme, ViewState,
};

pub const RING_LIMIT: usize = 300;
pub const NOTICE_MS: i64 = 5000;
pub const CONFIRM_DELAY_MS: i64 = 300;
pub const SPINNER_MS: i64 = 120;
pub const MIN_COLUMNS: u16 = 60;
pub const MIN_ROWS: u16 = 16;
const TRUNCATE_NOTICE: usize = 200;

pub const PANEL_ORDER: [PanelId; 5] = [
    PanelId::Agents,
    PanelId::Pipeline,
    PanelId::Queue,
    PanelId::Findings,
    PanelId::Work,
];

/// The functions the app calls in the model and view layers; tests replace them with fakes.
#[derive(Clone, Copy)]
pub struct Hooks {
    pub build_model: fn(&Value, i64, Option<i64>) -> DashModel,
    pub build_frame: fn(&DashModel, &ViewState, &Theme) -> Frame,
    pub help_overlay: fn(Size, &Theme) -> Overlay,
    pub confirm_overlay: fn(&DashAction, Size, &Theme) -> Overlay,
    pub observe_overlay: fn(&PeekView, Size, &Theme) -> Overlay,
    pub cell_width: fn(&str) -> usize,
}

impl Default for Hooks {
    fn default() -> Self {
        Hooks {
            build_model: crate::model::build_dash_model,
            build_frame: crate::view::build_frame,
            help_overlay: crate::view::help_overlay,
            confirm_overlay: crate::view::confirm_overlay,
            observe_overlay: crate::view::observe_overlay,
            cell_width: crate::view::cell_width,
        }
    }
}

/// What a key or an event asks the runtime to do outside the state.
#[derive(Clone, Debug, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum Effect {
    Quit,
    Bell,
    PollNow,
    SetPaused(bool),
    SetIntervalSeconds(u32),
    Call(DashAction),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Prompt {
    pub action: DashAction,
    pub opened_at: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub id: Option<String>,
    pub index: usize,
}

/// The overlay that is up, if any: a peek over a prompt over help.
#[derive(Clone, Debug, PartialEq)]
pub enum OverlayKind {
    Peek(PeekView),
    Prompt(DashAction),
    Help,
}

pub struct AppState {
    hooks: Hooks,
    worker_limit: Option<i64>,
    reduced_motion: bool,
    pub confirm_delay_ms: i64,
    pub model: Option<DashModel>,
    /// Counts model rebuilds.
    pub model_rev: u64,
    pub link: Link,
    changes: ChangeState,
    pub rings: Rings,
    /// Bumped whenever the rings change.
    pub rings_rev: u64,
    pub focus: PanelId,
    selection: [Selection; 5],
    pub problems_only: bool,
    pub show_all_ended: bool,
    pub interval_seconds: u32,
    pub paused: bool,
    pub help: bool,
    pub prompt: Option<Prompt>,
    pub peek: Option<PeekView>,
    pub notice: Option<String>,
    notice_until: Option<i64>,
    pub busy: bool,
    last_ok_ms: Option<i64>,
    seen_signals: Option<HashSet<String>>,
    /// Panels the last frame showed, in focus order.
    pub shown: Vec<PanelId>,
    pub tick: u64,
}

fn panel_index(panel: PanelId) -> usize {
    PANEL_ORDER.iter().position(|p| *p == panel).unwrap_or(0)
}

/// Where the selection is now: the remembered row if it is still shown, else the old position kept inside the list.
pub fn resolve_selection(ids: &[String], selection: &Selection) -> usize {
    if ids.is_empty() {
        return 0;
    }
    let at = selection
        .id
        .as_ref()
        .and_then(|id| ids.iter().position(|i| i == id));
    at.unwrap_or_else(|| selection.index.min(ids.len() - 1))
}

/// The row ids each panel shows, in display order.
pub fn row_ids(model: &DashModel, problems_only: bool, show_all_ended: bool) -> [Vec<String>; 5] {
    [
        visible_agents(model, show_all_ended)
            .iter()
            .map(|a| a.id.clone())
            .collect(),
        pipeline_items(model).iter().map(|i| i.id.clone()).collect(),
        queue_rows(model, problems_only)
            .iter()
            .map(|m| m.id.clone())
            .collect(),
        model.findings.iter().map(|f| f.id.clone()).collect(),
        model.work.iter().map(|w| w.id.clone()).collect(),
    ]
}

/// `HH:MM:SS` in local time.
pub fn format_clock(ms: i64) -> String {
    let seconds = ms.div_euclid(1000) as libc::time_t;
    // SAFETY: `seconds` and `parts` are valid for the call; localtime_r fills `parts` and keeps no reference.
    let parts = unsafe {
        let mut parts: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&seconds, &mut parts).is_null() {
            return "00:00:00".to_string();
        }
        parts
    };
    format!(
        "{:02}:{:02}:{:02}",
        parts.tm_hour, parts.tm_min, parts.tm_sec
    )
}

/// Elapsed time as `12s`, `3m`, `2h` or `4d`.
pub fn age_since(then_ms: i64, now_ms: i64) -> String {
    let seconds = (now_ms - then_ms).div_euclid(1000).max(0);
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else if seconds < 86_400 {
        format!("{}h", seconds / 3600)
    } else {
        format!("{}d", seconds / 86_400)
    }
}

fn list<'a>(status: &'a Value, key: &str) -> &'a [Value] {
    status
        .get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn text_of(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => "undefined".to_string(),
        Some(other) => other.to_string(),
    }
}

/// The events that make the bell ring when one is new: a notified message, an input clear, a stuck message, a lost
/// agent or an escalated finding.
pub fn signals_of(status: &Value) -> HashSet<String> {
    let mut signals = HashSet::new();
    for message in list(status, "messages") {
        if let Some(at) = message.get("lastNotifiedAt").filter(|v| !v.is_null()) {
            signals.insert(format!(
                "notified:{}:{}",
                text_of(message.get("messageId")),
                text_of(Some(at))
            ));
        }
    }
    for clear in list(status, "inputClears") {
        signals.insert(format!("clear:{}", text_of(clear.get("clearId"))));
    }
    for stuck in list(status, "stuck") {
        signals.insert(format!(
            "stuck:{}:{}",
            text_of(stuck.get("messageId")),
            text_of(stuck.get("reason"))
        ));
    }
    for id in list(status, "lostAgentIds") {
        signals.insert(format!("lost:{}", text_of(Some(id))));
    }
    for finding in list(status, "agentFindings") {
        if finding.get("state").and_then(Value::as_str) == Some("escalated") {
            signals.insert(format!(
                "finding-escalated:{}",
                text_of(finding.get("findingId"))
            ));
        }
    }
    signals
}

fn push_sample(ring: &mut Vec<f64>, value: f64) {
    ring.push(value);
    if ring.len() > RING_LIMIT {
        ring.drain(..ring.len() - RING_LIMIT);
    }
}

impl AppState {
    pub fn new(
        hooks: Hooks,
        interval_seconds: u32,
        worker_limit: Option<i64>,
        reduced_motion: bool,
    ) -> Self {
        AppState {
            hooks,
            worker_limit,
            reduced_motion,
            confirm_delay_ms: CONFIRM_DELAY_MS,
            model: None,
            model_rev: 0,
            link: Link::Starting,
            changes: ChangeState::default(),
            rings: Rings::default(),
            rings_rev: 0,
            focus: PanelId::Queue,
            selection: Default::default(),
            problems_only: false,
            show_all_ended: false,
            interval_seconds,
            paused: false,
            help: false,
            prompt: None,
            peek: None,
            notice: None,
            notice_until: None,
            busy: false,
            last_ok_ms: None,
            seen_signals: None,
            shown: PANEL_ORDER.to_vec(),
            tick: 0,
        }
    }

    pub fn working(&self) -> bool {
        self.model.as_ref().is_some_and(|m| m.counts.working > 0)
    }

    /// Whether the spinner runs: agents are working and motion is allowed.
    pub fn spinner_runs(&self) -> bool {
        !self.reduced_motion && self.working()
    }

    pub fn set_notice(&mut self, text: impl Into<String>, now: i64) {
        self.notice = Some(text.into());
        self.notice_until = Some(now + NOTICE_MS);
    }

    /// When the notice goes away, if there is one.
    pub fn notice_deadline(&self) -> Option<i64> {
        self.notice.as_ref().and(self.notice_until)
    }

    /// Clears an expired notice; true when something changed.
    pub fn expire(&mut self, now: i64) -> bool {
        match self.notice_deadline() {
            Some(until) if now >= until => {
                self.notice = None;
                self.notice_until = None;
                true
            }
            _ => false,
        }
    }

    fn truncate(&self, text: &str, width: usize) -> String {
        let measure = self.hooks.cell_width;
        if width == 0 {
            return String::new();
        }
        if measure(text) <= width {
            return text.to_string();
        }
        let mut used = 0;
        let mut out = String::new();
        for c in text.chars() {
            let w = measure(c.encode_utf8(&mut [0; 4]));
            if used + w > width - 1 {
                break;
            }
            out.push(c);
            used += w;
        }
        out.push('…');
        out
    }

    fn ids(&self) -> Option<[Vec<String>; 5]> {
        self.model
            .as_ref()
            .map(|m| row_ids(m, self.problems_only, self.show_all_ended))
    }

    fn index_of(&self, panel: PanelId) -> usize {
        self.ids().map_or(0, |ids| {
            resolve_selection(
                &ids[panel_index(panel)],
                &self.selection[panel_index(panel)],
            )
        })
    }

    pub fn can_act(&self) -> bool {
        self.link == Link::Ok && !self.paused && !self.busy && self.model.is_some()
    }

    fn move_selection(&mut self, panel: PanelId, delta: isize) {
        let list = self
            .ids()
            .map(|mut ids| std::mem::take(&mut ids[panel_index(panel)]))
            .unwrap_or_default();
        let slot = panel_index(panel);
        let at = resolve_selection(&list, &self.selection[slot]) as isize;
        let last = list.len().saturating_sub(1) as isize;
        let index = (at + delta).max(0).min(last.max(0)) as usize;
        self.selection[slot] = Selection {
            id: list.get(index).cloned(),
            index,
        };
    }

    /// The overlay that is up.
    pub fn overlay(&self) -> Option<OverlayKind> {
        if let Some(peek) = &self.peek {
            Some(OverlayKind::Peek(peek.clone()))
        } else if let Some(prompt) = &self.prompt {
            Some(OverlayKind::Prompt(prompt.action.clone()))
        } else if self.help {
            Some(OverlayKind::Help)
        } else {
            None
        }
    }

    fn start(&mut self, action: DashAction) -> Vec<Effect> {
        self.busy = true;
        vec![Effect::Call(action)]
    }

    pub fn on_key(&mut self, key: Key, now: i64) -> Vec<Effect> {
        if self.peek.is_some() {
            if matches!(key, Key::Esc | Key::Char('q')) {
                self.peek = None;
            }
            return Vec::new();
        }
        if let Some(prompt) = &self.prompt {
            if key == Key::Char('y') {
                if now - prompt.opened_at >= self.confirm_delay_ms {
                    let action = prompt.action.clone();
                    self.prompt = None;
                    return self.start(action);
                }
            } else {
                self.prompt = None;
                self.set_notice("cancelled", now);
            }
            return Vec::new();
        }
        if matches!(key, Key::Char('q') | Key::CtrlC) {
            return vec![Effect::Quit];
        }
        if key == Key::Char('?') {
            self.help = !self.help;
            return Vec::new();
        }
        if self.help {
            self.help = false;
            return Vec::new();
        }
        match key {
            Key::Char('p') => {
                self.paused = !self.paused;
                return vec![Effect::SetPaused(self.paused)];
            }
            Key::Char('r') => return vec![Effect::PollNow],
            Key::Char(c @ ('-' | '+' | '=')) => {
                let next = step_interval(self.interval_seconds, c == '-');
                if next == self.interval_seconds {
                    return Vec::new();
                }
                self.interval_seconds = next;
                return vec![Effect::SetIntervalSeconds(next)];
            }
            _ => {}
        }
        if let Key::Char(c @ '1'..='5') = key {
            let jump = PANEL_ORDER[c as usize - '1' as usize];
            if self.shown.contains(&jump) {
                self.focus = jump;
                return Vec::new();
            }
        }
        if matches!(key, Key::Tab | Key::BackTab) {
            let count = self.shown.len() as isize;
            if count > 0 {
                let at = self
                    .shown
                    .iter()
                    .position(|p| *p == self.focus)
                    .map_or(-1, |i| i as isize);
                let step = if key == Key::BackTab { -1 } else { 1 };
                self.focus = self.shown[((at + step + count) % count) as usize];
            }
            return Vec::new();
        }
        if matches!(key, Key::Up | Key::Down | Key::Char('j' | 'k')) {
            let delta = if matches!(key, Key::Up | Key::Char('k')) {
                -1
            } else {
                1
            };
            self.move_selection(self.focus, delta);
            return Vec::new();
        }
        if key == Key::Char('f') && self.focus == PanelId::Queue {
            self.problems_only = !self.problems_only;
            return Vec::new();
        }
        if key == Key::Char('e') && self.focus == PanelId::Agents {
            self.show_all_ended = !self.show_all_ended;
            return Vec::new();
        }
        if !self.can_act() {
            return Vec::new();
        }
        let Some(model) = &self.model else {
            return Vec::new();
        };
        if key == Key::Char('o') && self.focus == PanelId::Agents {
            let at = self.index_of(PanelId::Agents);
            let Some(agent) = visible_agents(model, self.show_all_ended).get(at).copied() else {
                return Vec::new();
            };
            return match observe_action(agent) {
                Some(action) => self.start(action),
                None => {
                    let text = format!(
                        "{} has ended; observe needs an active agent",
                        agent.agent_id
                    );
                    self.set_notice(text, now);
                    Vec::new()
                }
            };
        }
        if self.focus == PanelId::Queue {
            let at = self.index_of(PanelId::Queue);
            let Some(message) = queue_rows(model, self.problems_only).get(at).copied() else {
                return Vec::new();
            };
            let Key::Char(c) = key else {
                return Vec::new();
            };
            let Some(decision) = Decision::ALL.into_iter().find(|d| d.key() == c.to_string())
            else {
                return Vec::new();
            };
            if !available_decisions(message).contains(&decision) {
                let text = format!(
                    "{} does not apply to a message in state {}",
                    decision.as_str(),
                    message.state
                );
                self.set_notice(text, now);
                return Vec::new();
            }
            self.prompt = Some(Prompt {
                action: DashAction::Resolve {
                    decision,
                    message: message.clone(),
                },
                opened_at: now,
            });
        }
        Vec::new()
    }

    pub fn on_poll(&mut self, event: PollEvent, now: i64) -> Vec<Effect> {
        let mut effects = Vec::new();
        match event {
            PollEvent::Failed { link } => self.link = link,
            PollEvent::Status { status, changed } => {
                self.last_ok_ms = Some(now);
                self.link = Link::Ok;
                if changed || self.model.is_none() {
                    let next = (self.hooks.build_model)(&status, now, self.worker_limit);
                    self.changes = track_changes(&self.changes, &next);
                    self.model = Some(next);
                    self.model_rev += 1;
                    let signals = signals_of(&status);
                    if !self.reduced_motion
                        && self
                            .seen_signals
                            .as_ref()
                            .is_some_and(|seen| signals.iter().any(|s| !seen.contains(s)))
                    {
                        effects.push(Effect::Bell);
                    }
                    self.seen_signals = Some(signals);
                    self.cancel_changed_prompt(now);
                }
                if let Some(model) = &self.model {
                    let sample = history_sample(model, now);
                    push_sample(&mut self.rings.unresolved, sample.unresolved);
                    push_sample(&mut self.rings.working, sample.working);
                    push_sample(&mut self.rings.oldest, sample.oldest_seconds);
                    self.rings_rev += 1;
                }
            }
        }
        effects
    }

    /// A prompt for a message that has gone or changed state is withdrawn.
    fn cancel_changed_prompt(&mut self, now: i64) {
        let (Some(prompt), Some(model)) = (&self.prompt, &self.model) else {
            return;
        };
        let DashAction::Resolve { message, .. } = &prompt.action else {
            return;
        };
        let same = model
            .queue
            .messages
            .iter()
            .find(|m| m.message_id == message.message_id)
            .is_some_and(|current| current.state == message.state);
        if !same {
            self.prompt = None;
            self.set_notice("cancelled: the message changed", now);
        }
    }

    /// The daemon's answer to an action.
    pub fn on_call_done(
        &mut self,
        action: &DashAction,
        result: CallResult,
        now: i64,
    ) -> Vec<Effect> {
        self.busy = false;
        match action {
            DashAction::Observe { agent_id } => match result {
                CallResult::Failed(message) => {
                    let text = self.truncate(&clean(&message), TRUNCATE_NOTICE);
                    self.set_notice(text, now);
                }
                CallResult::Ok(value) => {
                    let agent_status = match value.get("agentStatus") {
                        None | Some(Value::Null) => "unknown".to_string(),
                        Some(Value::String(s)) => clean(s),
                        Some(other) => clean(&other.to_string()),
                    };
                    let text = value
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    self.peek = Some(PeekView {
                        agent_id: agent_id.clone(),
                        agent_status,
                        text,
                    });
                }
            },
            DashAction::Resolve { decision, message } => {
                let text = match result {
                    CallResult::Ok(value) => {
                        let state = match value.get("state") {
                            None | Some(Value::Null) => "done".to_string(),
                            Some(Value::String(s)) => clean(s),
                            Some(other) => clean(&other.to_string()),
                        };
                        format!("{} {}: {state}", decision.as_str(), message.message_id)
                    }
                    CallResult::Failed(message) => self.truncate(&clean(&message), TRUNCATE_NOTICE),
                };
                self.set_notice(text, now);
                return vec![Effect::PollNow];
            }
        }
        Vec::new()
    }

    /// The spinner frame advances.
    pub fn advance_spinner(&mut self) {
        self.tick += 1;
    }

    /// The text shown instead of the dashboard, if any.
    pub fn placeholder(&self, size: Size) -> Option<String> {
        if size.columns < MIN_COLUMNS || size.rows < MIN_ROWS {
            return Some(format!(
                "terminal too small (need {MIN_COLUMNS}x{MIN_ROWS}, have {}x{})",
                size.columns, size.rows
            ));
        }
        if self.model.is_none() {
            return Some(
                if self.link == Link::Down {
                    "controller not answering, retrying"
                } else {
                    "connecting to the controller..."
                }
                .to_string(),
            );
        }
        None
    }

    /// The view state for one draw. `with_rings` is false when it is only compared with the last one.
    pub fn view_state(&self, size: Size, now: i64, with_rings: bool) -> ViewState {
        let ids = self.ids();
        let selected_of = |panel: PanelId| {
            ids.as_ref().map_or(0, |ids| {
                resolve_selection(
                    &ids[panel_index(panel)],
                    &self.selection[panel_index(panel)],
                )
            })
        };
        ViewState {
            size,
            focus: self.focus,
            selected: Selected {
                agents: selected_of(PanelId::Agents),
                pipeline: selected_of(PanelId::Pipeline),
                queue: selected_of(PanelId::Queue),
                findings: selected_of(PanelId::Findings),
                work: selected_of(PanelId::Work),
            },
            problems_only: self.problems_only,
            show_all_ended: self.show_all_ended,
            paused: self.paused,
            link: self.link,
            link_age: self
                .last_ok_ms
                .map_or_else(|| "-".to_string(), |ok| age_since(ok, now)),
            clock: format_clock(now),
            interval_seconds: self.interval_seconds,
            now_ms: now,
            tick: self.tick,
            rings: if with_rings {
                self.rings.clone()
            } else {
                Rings::default()
            },
            highlight: self.changes.highlight.clone(),
            notice: if self.busy && self.notice.is_none() {
                Some("working...".to_string())
            } else {
                self.notice.clone()
            },
        }
    }
}
