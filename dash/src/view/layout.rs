//! Pure layout decisions: terminal mode, column split, how rows are shared between panels, and which table columns fit (`layout.ts`).
use super::types::PanelId;

pub const MIN_COLUMNS: usize = 60;
pub const MIN_ROWS: usize = 16;
pub const WIDE_COLUMNS: usize = 100;
/// Rows below which a wrapped header reason is cut with an ellipsis instead.
pub const WRAP_REASON_ROWS: usize = 20;
/// Below this many rows the task summary shares a status row instead of taking its own.
pub const SHARED_TASK_ROWS: usize = 30;
/// The fewest cells of task summary worth sharing a row for.
pub const MIN_SHARED_SUMMARY: usize = 24;
/// Panel chrome: top and bottom border.
pub const CHROME_ROWS: usize = 2;
/// Spare rows given to a graph never exceed this.
pub const MAX_GRAPH_ROWS: usize = 24;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutMode {
    Tiny,
    Narrow,
    Wide,
}

#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub mode: LayoutMode,
}

pub fn layout_for(columns: usize, rows: usize) -> Layout {
    let mode = if columns < MIN_COLUMNS || rows < MIN_ROWS {
        LayoutMode::Tiny
    } else if columns >= WIDE_COLUMNS {
        LayoutMode::Wide
    } else {
        LayoutMode::Narrow
    };
    Layout { mode }
}

/// Panels per column: stacked in one column, or agents and pipeline left, the rest right.
/// While the queue is collapsed to a stub the pipeline joins the right column under it,
/// so agents get the whole left column.
pub fn columns_of(
    mode: LayoutMode,
    panels: &[PanelId],
    queue_collapsed: bool,
) -> Vec<Vec<PanelId>> {
    if mode != LayoutMode::Wide {
        return vec![panels.to_vec()];
    }
    let on_left = |p: PanelId| p == PanelId::Agents || (p == PanelId::Pipeline && !queue_collapsed);
    let left: Vec<PanelId> = panels.iter().copied().filter(|&p| on_left(p)).collect();
    let mut right: Vec<PanelId> = panels.iter().copied().filter(|&p| !on_left(p)).collect();
    if queue_collapsed {
        let rank = |p: &PanelId| match p {
            PanelId::Queue => 0,
            PanelId::Pipeline => 1,
            _ => 2,
        };
        // A stable sort, as in the Node implementation.
        right.sort_by_key(rank);
    }
    vec![left, right]
}

/// Widths of the columns: the left takes the rounded-down half.
pub fn column_widths(columns: usize, mode: LayoutMode) -> Vec<usize> {
    if mode != LayoutMode::Wide {
        return vec![columns];
    }
    let left = columns / 2;
    vec![left, columns.saturating_sub(left)]
}

#[derive(Clone, Copy, Debug)]
pub struct PanelWish {
    pub id: PanelId,
    /// Smallest total height, borders included.
    pub min: usize,
    /// Height that shows all of the panel's content, borders included.
    pub want: usize,
    /// Share of spare rows when panels compete.
    pub weight: f64,
    /// Whether the panel can use more than `want` rows (graphs, detail).
    pub stretch: bool,
    /// A stub that never takes spare rows, not even as blank interior rows.
    pub collapsed: bool,
}

/// Panels dropped, in this order, when the column cannot pay every minimum.
const DROP_ORDER: [PanelId; 3] = [PanelId::Work, PanelId::Findings, PanelId::Pipeline];
/// Who receives rows left over after every wish is met, after the focused panel.
const STRETCH_ORDER: [PanelId; 2] = [PanelId::Queue, PanelId::Agents];

/// Shares `rows` between panels in one column. Every shown panel first gets its minimum; then each spare row goes to
/// the panel with the best weight / (extra + 1) that still wants more (D'Hondt), so short panels reach their full
/// content before long ones take more; rows still left over go to the focused panel if it can stretch, else the first
/// stretching panel, else the last panel (as blank interior rows). Returns the shown panels in order with their
/// total heights.
pub fn fill_rows(rows: usize, wishes: &[PanelWish], focus: PanelId) -> Vec<(PanelId, usize)> {
    let mut shown: Vec<PanelWish> = wishes.to_vec();
    let min_total = |shown: &[PanelWish]| shown.iter().map(|w| w.min).sum::<usize>();
    for id in DROP_ORDER {
        if min_total(&shown) <= rows {
            break;
        }
        shown.retain(|w| w.id != id);
    }
    let mut heights: Vec<(PanelId, usize)> = shown.iter().map(|w| (w.id, w.min)).collect();
    let mut spare = rows as i64 - min_total(&shown) as i64;
    while spare > 0 {
        let mut best: Option<usize> = None;
        let mut best_score = 0.0;
        for (i, w) in shown.iter().enumerate() {
            if heights[i].1 >= w.want {
                continue;
            }
            let score = w.weight / ((heights[i].1.saturating_sub(w.min)) as f64 + 1.0);
            if score > best_score {
                best = Some(i);
                best_score = score;
            }
        }
        let Some(best) = best else { break };
        heights[best].1 += 1;
        spare -= 1;
    }
    if spare > 0 && !shown.is_empty() {
        let stretching: Vec<usize> = (0..shown.len()).filter(|&i| shown[i].stretch).collect();
        let target = stretching
            .iter()
            .copied()
            .find(|&i| shown[i].id == focus)
            .or_else(|| {
                STRETCH_ORDER
                    .iter()
                    .find_map(|id| stretching.iter().copied().find(|&i| shown[i].id == *id))
            })
            .or_else(|| (0..shown.len()).rev().find(|&i| !shown[i].collapsed))
            .unwrap_or(shown.len() - 1);
        heights[target].1 += spare as usize;
    }
    heights
}

/// Heights of a panel in a `fill_rows` result.
pub fn height_of(heights: &[(PanelId, usize)], id: PanelId) -> Option<usize> {
    heights.iter().find(|(p, _)| *p == id).map(|(_, h)| *h)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    pub start: usize,
    pub end: usize,
    pub hidden: usize,
}

/// The slice of `count` rows that fits `capacity` and keeps `cursor` visible.
pub fn window_of(count: usize, cursor: usize, capacity: usize) -> Window {
    if capacity == 0 {
        return Window {
            start: 0,
            end: 0,
            hidden: count,
        };
    }
    if capacity >= count {
        return Window {
            start: 0,
            end: count,
            hidden: 0,
        };
    }
    let start = (cursor + 1)
        .saturating_sub(capacity)
        .min(count.saturating_sub(capacity));
    Window {
        start,
        end: start + capacity,
        hidden: count.saturating_sub(capacity),
    }
}

/// Optional table columns by the inner content width of a panel (`width - 4`).
#[derive(Clone, Copy, Debug)]
pub enum OptionalColumn {
    AgentsPane,
    QueueNotified,
    AgentsRole,
    AgentsActivity,
    QueueClearsTab,
    PipelineTab,
}

pub fn fits(content_width: usize, column: OptionalColumn) -> bool {
    content_width
        >= match column {
            OptionalColumn::AgentsPane => 60,
            OptionalColumn::QueueNotified => 56,
            OptionalColumn::AgentsRole => 46,
            OptionalColumn::AgentsActivity => 40,
            OptionalColumn::QueueClearsTab => 51,
            OptionalColumn::PipelineTab => 45,
        }
}

/// How rows inside a panel are shared. Inputs are the body rows left after the borders.
#[derive(Clone, Copy, Debug)]
pub struct QueueSections {
    pub header: usize,
    pub list: usize,
    pub detail: usize,
    /// Rows for graphs including their caption lines.
    pub graphs: usize,
}

pub const DETAIL_ROWS: usize = 4;
pub const GRAPH_MIN_ROWS: usize = 4;
pub const TWO_GRAPHS_ROWS: usize = 14;

pub fn queue_sections(body: usize, messages: usize, has_selection: bool) -> QueueSections {
    let header = if body >= 3 { 1 } else { 0 };
    let avail = body as i64 - header as i64;
    let spare = avail - messages as i64;
    let detail = if has_selection && spare > DETAIL_ROWS as i64 {
        DETAIL_ROWS
    } else {
        0
    };
    let graph_spare = spare - detail as i64;
    let graphs = if graph_spare >= GRAPH_MIN_ROWS as i64 {
        graph_spare as usize
    } else {
        0
    };
    let list = (messages as i64).min(avail - detail as i64 - graphs as i64);
    QueueSections {
        header,
        list: list.max(0) as usize,
        detail,
        graphs,
    }
}

#[derive(Clone, Copy, Debug)]
pub struct AgentSections {
    pub header: usize,
    /// Rows for agents, active first and then ended, scrolled as one list.
    pub rows: usize,
    pub graph: usize,
}

pub fn agent_sections(body: usize, agents: usize) -> AgentSections {
    let header = if body >= 3 { 1 } else { 0 };
    let avail = body.saturating_sub(header);
    let rows = agents.max(1).min(avail);
    let left = avail.saturating_sub(rows);
    AgentSections {
        header,
        rows,
        graph: if left >= 3 { left } else { 0 },
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PipelineSections {
    pub flow: usize,
    /// Stage bars, or the one-line history summary in the compact form.
    pub stages: usize,
    pub compact_summary: bool,
    /// A blank row between the stage bars, so full-block bars do not merge into one slab.
    pub gapped: bool,
    pub header: usize,
    pub items: usize,
}

/// The collapsed queue: borders and one line.
pub const COLLAPSED_QUEUE_ROWS: usize = 3;

/// Rows a gapped stage block takes: three bars and the two blank rows between them.
pub const GAPPED_STAGE_ROWS: usize = 5;

pub fn pipeline_sections(body: usize, items: usize) -> PipelineSections {
    if body <= 2 {
        return PipelineSections {
            flow: body.min(1),
            stages: 0,
            compact_summary: body == 2,
            gapped: false,
            header: 0,
            items: 0,
        };
    }
    let gapped = body as i64 - 1 - GAPPED_STAGE_ROWS as i64 >= 3;
    let stages = if gapped {
        GAPPED_STAGE_ROWS
    } else {
        3.min(body.saturating_sub(1))
    };
    let rest = body.saturating_sub(1 + stages);
    if rest < 3 {
        return PipelineSections {
            flow: 1,
            stages,
            compact_summary: false,
            gapped: false,
            header: 0,
            items: 0,
        };
    }
    PipelineSections {
        flow: 1,
        stages,
        compact_summary: false,
        gapped,
        header: 1,
        items: items.min(rest.saturating_sub(1)),
    }
}
