//! The five panels and the table and border assembly they share (`view.ts`).
mod agents;
mod findings;
mod pipeline;
mod queue;
mod work;

use super::border::{bottom_border, top_border, BorderColors, BottomBorder, Tab, Thumb, TopBorder};
use super::format::{cell_width, truncate};
use super::glyphs::Glyphs;
use super::layout::{PanelWish, Window, CHROME_ROWS};
use super::lines::{fit_line, line_width, merge_spans, plain, span, style_line, Style};
use super::theme::color_of_state;
use super::types::{ColorRole, Line, PanelId, Span, Theme, ViewState};
use crate::model::{pipeline_items, queue_collapsed, queue_rows, visible_agents, DashModel};

pub use super::layout::COLLAPSED_QUEUE_ROWS;

pub struct Ctx<'a> {
    pub model: &'a DashModel,
    pub view: &'a ViewState,
    pub theme: &'a Theme,
    pub g: Glyphs,
    pub w: usize,
    pub h: usize,
    pub focused: bool,
}

impl Ctx<'_> {
    pub fn body(&self) -> usize {
        self.h.saturating_sub(CHROME_ROWS)
    }

    pub fn content(&self) -> usize {
        self.w.saturating_sub(4)
    }

    pub fn color(&self, role: ColorRole) -> Option<String> {
        self.theme.color(role)
    }

    pub fn highlighted(&self, key: &str) -> bool {
        self.view.highlight.iter().any(|(k, _)| k == key)
    }
}

pub const NUMBERS: [(PanelId, usize); 5] = [
    (PanelId::Agents, 1),
    (PanelId::Pipeline, 2),
    (PanelId::Queue, 3),
    (PanelId::Findings, 4),
    (PanelId::Work, 5),
];

pub fn panel_title(id: PanelId) -> &'static str {
    match id {
        PanelId::Agents => "agents",
        PanelId::Pipeline => "pipeline",
        PanelId::Queue => "queue",
        PanelId::Findings => "findings",
        PanelId::Work => "work items",
    }
}

pub fn panel_number(id: PanelId) -> usize {
    NUMBERS
        .iter()
        .find(|(p, _)| *p == id)
        .map(|(_, n)| *n)
        .unwrap_or(0)
}

// ---------------------------------------------------------------- tables

#[derive(Clone, Debug)]
pub struct Col {
    pub key: &'static str,
    pub title: &'static str,
    pub width: usize,
    pub right: bool,
    pub flex: bool,
}

impl Col {
    pub fn new(key: &'static str, title: &'static str, width: usize) -> Col {
        Col {
            key,
            title,
            width,
            right: false,
            flex: false,
        }
    }

    pub fn right(mut self) -> Col {
        self.right = true;
        self
    }

    pub fn flex(mut self) -> Col {
        self.flex = true;
        self
    }
}

pub fn resolve_cols(content_width: usize, cols: Vec<Col>) -> Vec<Col> {
    let fixed: i64 = cols
        .iter()
        .filter(|c| !c.flex)
        .map(|c| c.width as i64)
        .sum::<i64>()
        + (cols.len() as i64 - 1);
    cols.into_iter()
        .map(|c| {
            if c.flex {
                Col {
                    width: (content_width as i64 - fixed).max(6) as usize,
                    ..c
                }
            } else {
                c
            }
        })
        .collect()
}

fn cell_line(value: &[Span], col: &Col) -> Line {
    if !col.right || line_width(value) >= col.width {
        return fit_line(value, col.width);
    }
    let mut out = vec![plain(" ".repeat(col.width - line_width(value)))];
    out.extend(value.iter().cloned());
    out
}

pub fn table_header(cols: &[Col], theme: &Theme) -> Line {
    let mut spans: Vec<Span> = Vec::new();
    for (i, c) in cols.iter().enumerate() {
        if i > 0 {
            spans.push(plain(" "));
        }
        let title = span(c.title, Style::color(theme.color(ColorRole::Dim)).bold());
        spans.extend(cell_line(&[title], c));
    }
    merge_spans(spans)
}

pub struct RowSpec {
    pub cells: Vec<(&'static str, Vec<Span>)>,
    pub selected: bool,
    pub changed: bool,
    pub dim: bool,
}

/// One body line of width `w - 2`: marker column, table cells, one pad cell.
pub fn table_row(ctx: &Ctx, cols: &[Col], row: RowSpec) -> Line {
    let (theme, g) = (ctx.theme, &ctx.g);
    let marker = if row.selected {
        span(
            g.selected,
            Style::color(theme.color(ColorRole::Bright)).bold(),
        )
    } else if row.changed && theme.reduced_motion {
        plain(g.changed)
    } else {
        plain(" ")
    };
    let none: Vec<Span> = vec![plain("")];
    let mut line: Line = vec![marker];
    for (i, c) in cols.iter().enumerate() {
        if i > 0 {
            line.push(plain(" "));
        }
        let value = row
            .cells
            .iter()
            .find(|(k, _)| *k == c.key)
            .map(|(_, v)| v)
            .unwrap_or(&none);
        line.extend(cell_line(value, c));
    }
    line.push(plain(" "));
    if row.selected {
        line = style_line(
            &line,
            &Style {
                bg: theme.color(ColorRole::SelectBg),
                bold: Some(true),
                ..Style::default()
            },
        );
    } else if row.changed && !theme.reduced_motion {
        for s in &mut line {
            s.bold = Some(true);
        }
    }
    if row.dim {
        line = style_line(
            &line,
            &Style {
                dim: Some(true),
                ..Style::default()
            },
        );
    }
    merge_spans(line)
}

pub fn text_line(ctx: &Ctx, text: &str, role: ColorRole) -> Line {
    fit_line(
        &[
            plain(" "),
            span(
                truncate(text, ctx.content()),
                Style::color(ctx.theme.color(role)),
            ),
        ],
        ctx.w.saturating_sub(2),
    )
}

pub fn indent(line: &[Span], w: usize) -> Line {
    let mut spans = vec![plain(" ")];
    spans.extend(line.iter().cloned());
    fit_line(&spans, w)
}

// ------------------------------------------------------------- assembly

fn border_role(id: PanelId, focused: bool) -> ColorRole {
    match (id, focused) {
        (PanelId::Agents, false) => ColorRole::BorderAgents,
        (PanelId::Agents, true) => ColorRole::BorderAgentsFocus,
        (PanelId::Pipeline, false) => ColorRole::BorderPipeline,
        (PanelId::Pipeline, true) => ColorRole::BorderPipelineFocus,
        (PanelId::Queue, false) => ColorRole::BorderQueue,
        (PanelId::Queue, true) => ColorRole::BorderQueueFocus,
        (PanelId::Findings, false) => ColorRole::BorderFindings,
        (PanelId::Findings, true) => ColorRole::BorderFindingsFocus,
        (PanelId::Work, false) => ColorRole::BorderWork,
        (PanelId::Work, true) => ColorRole::BorderWorkFocus,
    }
}

pub struct PanelSpec {
    pub id: PanelId,
    pub tabs: Vec<Tab>,
    pub bottom_left: Vec<Tab>,
    pub bottom_right: Vec<Tab>,
    pub body: Vec<Line>,
    pub thumb: Option<Thumb>,
    /// Body row where the thumb range starts.
    pub thumb_top: usize,
}

pub fn assemble(ctx: &Ctx, spec: PanelSpec) -> Vec<Line> {
    let (g, theme, w, h, focused) = (&ctx.g, ctx.theme, ctx.w, ctx.h, ctx.focused);
    let border = theme.color(border_role(spec.id, focused));
    let edge = if focused { g.edge_focused } else { g.edge };
    let rows = h.saturating_sub(CHROME_ROWS);
    let mut out: Vec<Line> = vec![top_border(
        w,
        &TopBorder {
            number: Some(panel_number(spec.id)),
            title: panel_title(spec.id).to_string(),
            tabs: spec.tabs.clone(),
            focused,
            ..TopBorder::default()
        },
        g,
        &BorderColors {
            border: border.clone(),
            title: theme.color(if focused {
                ColorRole::Bright
            } else {
                ColorRole::Fg
            }),
        },
    )];
    let empty: Line = Vec::new();
    for r in 0..rows {
        let inner = fit_line(spec.body.get(r).unwrap_or(&empty), w.saturating_sub(2));
        let in_thumb = spec.thumb.is_some_and(|t| {
            r >= spec.thumb_top + t.start && r < spec.thumb_top + t.start + t.size
        });
        let mut parts = vec![span(edge.v, Style::color(border.clone()))];
        parts.extend(inner);
        parts.push(if in_thumb {
            span(g.thumb, Style::color(theme.color(ColorRole::Fg)))
        } else {
            span(edge.v, Style::color(border.clone()))
        });
        out.push(merge_spans(parts));
    }
    out.push(bottom_border(
        w,
        &BottomBorder {
            left: spec.bottom_left,
            right: spec.bottom_right,
            focused,
        },
        g,
        &border,
    ));
    out.truncate(h);
    out
}

/// The position tab of a panel's bottom border. A focused panel has a cursor, so it shows `cursor/total`. An
/// unfocused panel has none; it shows the visible range `first-last/total`, and only when rows are hidden.
pub fn counter(ctx: &Ctx, selected: usize, total: usize, win: Window) -> Vec<Tab> {
    let info = ctx.theme.color(ColorRole::Info);
    if total == 0 {
        return Vec::new();
    }
    if ctx.focused {
        return vec![Tab::new(format!("{}/{total}", selected + 1), info)];
    }
    if win.end - win.start >= total {
        return Vec::new();
    }
    vec![Tab::new(
        format!("{}-{}/{total}", win.start + 1, win.end),
        info,
    )]
}

pub fn widest<'a>(words: impl IntoIterator<Item = &'a str>, floor: usize) -> usize {
    words.into_iter().fold(floor, |n, w| n.max(cell_width(w)))
}

pub fn state_span(ctx: &Ctx, text: &str) -> Vec<Span> {
    vec![span(
        text,
        Style::color(ctx.theme.color(color_of_state(text).role())),
    )]
}

pub fn one(s: Span) -> Vec<Span> {
    vec![s]
}

/// A number as JavaScript prints it: whole values without a fraction.
pub fn num(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

pub fn render(id: PanelId, ctx: &Ctx) -> Vec<Line> {
    match id {
        PanelId::Agents => agents::panel(ctx),
        PanelId::Pipeline => pipeline::panel(ctx),
        PanelId::Queue => queue::panel(ctx),
        PanelId::Findings => findings::panel(ctx),
        PanelId::Work => work::panel(ctx),
    }
}

pub fn wish_for(id: PanelId, model: &DashModel, view: &ViewState) -> PanelWish {
    let count = |n: usize| n.max(1);
    let base = PanelWish {
        id,
        min: 3,
        want: 0,
        weight: 1.0,
        stretch: false,
        collapsed: false,
    };
    match id {
        PanelId::Agents => PanelWish {
            want: CHROME_ROWS + 1 + count(visible_agents(model, view.show_all_ended).len()),
            weight: 4.0,
            stretch: true,
            ..base
        },
        PanelId::Pipeline => PanelWish {
            want: CHROME_ROWS + 1 + 3 + 2 + 1 + pipeline_items(model).len(),
            weight: 2.0,
            ..base
        },
        PanelId::Queue => {
            if queue_collapsed(model) {
                PanelWish {
                    min: COLLAPSED_QUEUE_ROWS,
                    want: COLLAPSED_QUEUE_ROWS,
                    collapsed: true,
                    ..base
                }
            } else {
                PanelWish {
                    want: CHROME_ROWS + 1 + count(queue_rows(model, view.problems_only).len()),
                    weight: 4.0,
                    stretch: true,
                    ..base
                }
            }
        }
        PanelId::Findings => PanelWish {
            want: CHROME_ROWS
                + 1
                + count(
                    model.findings.iter().filter(|f| !f.stale).count()
                        + usize::from(model.findings.iter().any(|f| f.stale)),
                ),
            ..base
        },
        PanelId::Work => PanelWish {
            want: CHROME_ROWS + 1 + count(model.work.len()),
            ..base
        },
    }
}
