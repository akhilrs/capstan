//! The shapes of the Node view layer (`view.ts`, `lines.ts`, `overlays.ts`, `theme.ts`).
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Size {
    pub columns: u16,
    pub rows: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PanelId {
    Agents,
    Pipeline,
    Queue,
    Findings,
    Work,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Link {
    Starting,
    Ok,
    Down,
    Toolarge,
}

/// Selected row index per panel, already resolved against the visible rows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Selected {
    pub agents: usize,
    pub pipeline: usize,
    pub queue: usize,
    pub findings: usize,
    pub work: usize,
}

/// History for the graphs, oldest sample first.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rings {
    pub unresolved: Vec<f64>,
    pub working: Vec<f64>,
    pub oldest: Vec<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewState {
    pub size: Size,
    pub focus: PanelId,
    pub selected: Selected,
    pub problems_only: bool,
    /// Agents panel: list every ended agent, not just the most recent few.
    pub show_all_ended: bool,
    pub paused: bool,
    pub link: Link,
    /// Time since the last good poll, for example `1s`.
    pub link_age: String,
    pub clock: String,
    pub interval_seconds: u32,
    pub now_ms: i64,
    pub tick: u64,
    pub rings: Rings,
    /// Row ids with the polls left to highlight each.
    pub highlight: Vec<(String, i64)>,
    pub notice: Option<String>,
}

/// A run of text with one style; an absent style field is the default.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Span {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bg: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bold: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dim: Option<bool>,
}

pub type Line = Vec<Span>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Frame {
    pub lines: Vec<Line>,
    /// Panels that fit on screen, in focus order.
    pub shown: Vec<PanelId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hint {
    pub key: String,
    pub label: String,
    pub priority: i64,
}

/// A box drawn over the dashboard at `top`, `left`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Overlay {
    pub lines: Vec<Line>,
    pub top: u16,
    pub left: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeekView {
    pub agent_id: String,
    pub agent_status: String,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ColorRole {
    #[serde(rename = "fg")]
    Fg,
    #[serde(rename = "bright")]
    Bright,
    #[serde(rename = "dim")]
    Dim,
    #[serde(rename = "ok")]
    Ok,
    #[serde(rename = "warn")]
    Warn,
    #[serde(rename = "bad")]
    Bad,
    #[serde(rename = "info")]
    Info,
    #[serde(rename = "selectBg")]
    SelectBg,
    #[serde(rename = "border.header")]
    BorderHeader,
    #[serde(rename = "border.agents")]
    BorderAgents,
    #[serde(rename = "border.agents.focus")]
    BorderAgentsFocus,
    #[serde(rename = "border.pipeline")]
    BorderPipeline,
    #[serde(rename = "border.pipeline.focus")]
    BorderPipelineFocus,
    #[serde(rename = "border.queue")]
    BorderQueue,
    #[serde(rename = "border.queue.focus")]
    BorderQueueFocus,
    #[serde(rename = "border.findings")]
    BorderFindings,
    #[serde(rename = "border.findings.focus")]
    BorderFindingsFocus,
    #[serde(rename = "border.work")]
    BorderWork,
    #[serde(rename = "border.work.focus")]
    BorderWorkFocus,
    #[serde(rename = "overlay")]
    Overlay,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeOptions {
    pub no_color: bool,
    pub reduced_motion: bool,
    pub ascii: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Theme {
    pub no_color: bool,
    pub reduced_motion: bool,
    pub ascii: bool,
}
