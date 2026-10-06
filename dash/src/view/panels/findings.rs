//! The findings panel.
use super::{
    assemble, counter, indent, resolve_cols, table_header, table_row, text_line, widest, Col, Ctx,
    PanelSpec, RowSpec,
};
use crate::model::FindingRow;
use crate::view::border::{thumb_range, Tab};
use crate::view::layout::window_of;
use crate::view::lines::{span, style_line, Style};
use crate::view::types::{ColorRole, Line, PanelId};

/// The longest severity word, `critical`.
const FINDING_SEVERITY_WIDTH: usize = 8;

/// The recorded reason, marked when the target is ended or not in the agent list.
fn finding_reason(f: &FindingRow) -> String {
    let marker = if f.target_state == "active" {
        String::new()
    } else {
        format!("(target {}) ", f.target_state)
    };
    // `trim()` in JavaScript strips whitespace at both ends.
    format!("{marker}{}", f.state_reason.as_deref().unwrap_or(""))
        .trim()
        .to_string()
}

pub fn panel(ctx: &Ctx) -> Vec<Line> {
    let (model, view, theme, g) = (ctx.model, ctx.view, ctx.theme, &ctx.g);
    let cw = ctx.content();
    let rows = &model.findings;
    let live: Vec<&FindingRow> = rows.iter().filter(|f| !f.stale).collect();
    let stale: Vec<&FindingRow> = rows.iter().filter(|f| f.stale).collect();
    let body0 = ctx.body();
    let header = usize::from(body0 >= 3);
    let avail = body0.saturating_sub(header);
    let caption_rows = usize::from(!stale.is_empty());
    let cols = resolve_cols(
        cw,
        vec![
            Col::new("glyph", " ", 1),
            Col::new("id", "ID", 6),
            Col::new(
                "target",
                "TARGET",
                widest(rows.iter().map(|f| f.target_agent_id.as_str()), 6),
            ),
            Col::new("sev", "SEV", FINDING_SEVERITY_WIDTH),
            Col::new("state", "STATE", 9),
            Col::new("int", "INT", 3),
            Col::new("reason", "REASON", 10).flex(),
        ],
    );
    // One window over live then stale rows (the caption takes a row of its own), so the visible rows
    // are always contiguous and the cursor stays in view in either block.
    let cursor = view.selected.findings;
    let cap = avail.saturating_sub(caption_rows);
    let win = window_of(rows.len(), cursor.min(rows.len().saturating_sub(1)), cap);
    let mut body: Vec<Line> = Vec::new();
    if header > 0 {
        body.push(indent(&table_header(&cols, theme), ctx.w.saturating_sub(2)));
    }
    if rows.is_empty() {
        body.push(text_line(ctx, "no open findings", ColorRole::Dim));
    }
    let row_of = |f: &FindingRow, index: usize| -> Line {
        let dim_role = || Style::color(theme.color(ColorRole::Dim));
        let fg = || Style::color(theme.color(ColorRole::Fg));
        let glyph = if f.stale {
            span(g.ended, dim_role())
        } else if f.needs_operator {
            span(g.attention, Style::color(theme.color(ColorRole::Bad)))
        } else {
            span(g.idle, Style::color(theme.color(ColorRole::Warn)))
        };
        let state = if f.stale {
            span(f.state.as_str(), dim_role())
        } else {
            span(
                if f.needs_operator {
                    "ESCALATED"
                } else {
                    f.state.as_str()
                },
                Style::color(theme.color(if f.needs_operator {
                    ColorRole::Bad
                } else {
                    ColorRole::Warn
                })),
            )
        };
        table_row(
            ctx,
            &cols,
            RowSpec {
                selected: ctx.focused && index == cursor,
                changed: ctx.highlighted(&format!("f:{}", f.id)),
                dim: f.stale,
                cells: vec![
                    ("glyph", vec![glyph]),
                    (
                        "id",
                        vec![span(f.finding_id.chars().take(6).collect::<String>(), fg())],
                    ),
                    ("target", vec![span(f.target_agent_id.as_str(), fg())]),
                    ("sev", vec![span(f.severity.as_str(), fg())]),
                    ("state", vec![state]),
                    (
                        "int",
                        vec![span(format!("{}/2", f.interventions), dim_role())],
                    ),
                    (
                        "reason",
                        vec![span(
                            finding_reason(f),
                            Style::color(theme.color(if f.stale {
                                ColorRole::Dim
                            } else {
                                ColorRole::Fg
                            })),
                        )],
                    ),
                ],
            },
        )
    };
    let live_end = win.end.min(live.len());
    for (i, f) in live.iter().enumerate().take(live_end).skip(win.start) {
        body.push(row_of(f, i));
    }
    if !stale.is_empty() && avail > 0 {
        body.push(style_line(
            &text_line(
                ctx,
                &format!(
                    "{} {} stale (target ended or unknown) {}",
                    g.rule.repeat(2),
                    stale.len(),
                    g.rule.repeat(2)
                ),
                ColorRole::Dim,
            ),
            &Style {
                dim: Some(true),
                ..Style::default()
            },
        ));
    }
    for i in win.start.max(live.len())..win.end {
        body.push(row_of(stale[i - live.len()], i));
    }
    let needs = live.iter().filter(|f| f.needs_operator).count();
    assemble(
        ctx,
        PanelSpec {
            id: PanelId::Findings,
            tabs: if needs > 0 {
                vec![Tab::new(
                    format!("{needs} needs operator"),
                    theme.color(ColorRole::Bad),
                )]
            } else {
                Vec::new()
            },
            bottom_left: Vec::new(),
            bottom_right: counter(ctx, cursor, rows.len(), win),
            body,
            thumb: thumb_range(rows.len(), cap, win.start, cap),
            thumb_top: header,
        },
    )
}
