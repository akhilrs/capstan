//! The queue panel.
use super::{
    assemble, counter, indent, num, resolve_cols, state_span, table_header, table_row, text_line,
    Col, Ctx, PanelSpec, RowSpec,
};
use crate::model::{available_decisions, queue_collapsed, queue_rows, MessageRow};
use crate::view::border::{thumb_range, Tab};
use crate::view::format::{age_detail, duration_text};
use crate::view::graph::{area_graph, graph_spans};
use crate::view::layout::{
    fits, queue_sections, window_of, OptionalColumn, QueueSections, MAX_GRAPH_ROWS, TWO_GRAPHS_ROWS,
};
use crate::view::lines::{blank_line, span, Style};
use crate::view::theme::color_of_state;
use crate::view::types::{ColorRole, Line, PanelId};

fn message_detail(m: &MessageRow) -> String {
    if let Some(problem) = &m.problem {
        return format!("! {problem}");
    }
    if let Some(reason) = &m.deferred_reason {
        return format!("deferred: {reason}");
    }
    m.state_reason.clone().unwrap_or_default()
}

fn max_of(series: &[f64], floor: f64) -> f64 {
    series.iter().copied().fold(floor, f64::max)
}

pub fn panel(ctx: &Ctx) -> Vec<Line> {
    let (model, view, theme, g) = (ctx.model, ctx.view, ctx.theme, &ctx.g);
    let cw = ctx.content();
    let collapsed = queue_collapsed(model);
    let rows = queue_rows(model, view.problems_only);
    let sec = if collapsed {
        QueueSections {
            header: 0,
            list: 0,
            detail: 0,
            graphs: 0,
        }
    } else {
        queue_sections(ctx.body(), rows.len().max(1), !rows.is_empty())
    };
    let mut cols = vec![
        Col::new("glyph", " ", 1),
        Col::new("seq", "SEQ", 5).right(),
        Col::new("to", "TO", 12),
        Col::new("state", "STATE", 8),
        Col::new("age", "AGE", 6).right(),
    ];
    if fits(cw, OptionalColumn::QueueNotified) {
        cols.push(Col::new("n", "N", 1));
    }
    cols.push(Col::new("detail", "DETAIL", 10).flex());
    let cols = resolve_cols(cw, cols);
    let mut body: Vec<Line> = Vec::new();
    if sec.header > 0 {
        body.push(indent(&table_header(&cols, theme), ctx.w.saturating_sub(2)));
    }
    let selected_index = view.selected.queue;
    let win = window_of(rows.len(), selected_index, sec.list);
    if collapsed {
        let max = max_of(&view.rings.unresolved, 0.0);
        let suffix = if max > 0.0 {
            format!(" {} max {} since dash start", g.rule, num(max))
        } else {
            String::new()
        };
        body.push(text_line(
            ctx,
            &format!("no unresolved messages{suffix}"),
            ColorRole::Dim,
        ));
    } else if rows.is_empty() {
        body.push(text_line(
            ctx,
            if view.problems_only {
                "no delivery problems"
            } else {
                "no unresolved messages"
            },
            ColorRole::Dim,
        ));
    }
    for (i, m) in rows[win.start..win.end].iter().enumerate() {
        let problem = m.problem.is_some();
        let fg = || Style::color(theme.color(ColorRole::Fg));
        body.push(table_row(
            ctx,
            &cols,
            RowSpec {
                selected: ctx.focused && win.start + i == selected_index,
                changed: ctx.highlighted(&format!("m:{}", m.id)),
                dim: false,
                cells: vec![
                    (
                        "glyph",
                        vec![if problem {
                            span(g.attention, Style::color(theme.color(ColorRole::Bad)))
                        } else {
                            span(
                                g.idle,
                                Style::color(theme.color(color_of_state(&m.state).role())),
                            )
                        }],
                    ),
                    ("seq", vec![span(format!("#{}", m.sequence), fg())]),
                    ("to", vec![span(m.recipient_agent_id.as_str(), fg())]),
                    ("state", state_span(ctx, &m.state)),
                    (
                        "age",
                        vec![span(age_detail(Some(&m.queued_at), view.now_ms), fg())],
                    ),
                    (
                        "n",
                        vec![span(
                            if m.notified {
                                g.notified
                            } else {
                                g.not_notified
                            },
                            Style::color(theme.color(ColorRole::Dim)),
                        )],
                    ),
                    (
                        "detail",
                        vec![span(
                            message_detail(m),
                            Style::color(theme.color(if problem {
                                ColorRole::Bad
                            } else {
                                ColorRole::Dim
                            })),
                        )],
                    ),
                ],
            },
        ));
    }
    let used = body.len();
    let selected = if rows.is_empty() {
        None
    } else {
        Some(rows[selected_index.min(rows.len() - 1)])
    };
    if sec.detail > 0 {
        while body.len() < used + (sec.list - sec.list.min(win.end - win.start)) {
            body.push(blank_line(ctx.w.saturating_sub(2)));
        }
        let caption = format!("{} selected ", g.rule.repeat(2));
        let caption_len = caption.chars().count();
        body.push(text_line(
            ctx,
            &format!("{caption}{}", g.rule.repeat(cw.saturating_sub(caption_len))),
            ColorRole::Dim,
        ));
        if let Some(selected) = selected {
            let decisions = available_decisions(selected);
            body.push(text_line(
                ctx,
                &format!(
                    "#{} to {}  {}  queued {} ago",
                    selected.sequence,
                    selected.recipient_agent_id,
                    selected.state,
                    age_detail(Some(&selected.queued_at), view.now_ms)
                ),
                ColorRole::Fg,
            ));
            body.push(text_line(
                ctx,
                &format!(
                    "message {}  {}",
                    selected.message_id,
                    if selected.notified {
                        "notified"
                    } else {
                        "not notified"
                    }
                ),
                ColorRole::Dim,
            ));
            let detail = message_detail(selected);
            let detail = if detail.is_empty() {
                format!("state {}", selected.state)
            } else {
                detail
            };
            let can = if decisions.is_empty() {
                "   no action in this state".to_string()
            } else {
                format!(
                    "   can: {}",
                    decisions
                        .iter()
                        .map(|d| d.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            body.push(text_line(
                ctx,
                &format!("{detail}{can}"),
                if selected.problem.is_some() {
                    ColorRole::Bad
                } else {
                    ColorRole::Dim
                },
            ));
        }
    }
    if sec.graphs > 0 {
        let two = sec.graphs >= TWO_GRAPHS_ROWS;
        let first = if two { sec.graphs / 2 } else { sec.graphs };
        let mut draw = |caption: String, rows_count: usize, series: &[f64], max: f64| {
            body.push(text_line(ctx, &caption, ColorRole::Dim));
            for spans in graph_spans(
                &area_graph(series, cw, MAX_GRAPH_ROWS.min(rows_count - 1), max, g.ascii),
                theme,
            ) {
                body.push(indent(&spans, ctx.w.saturating_sub(2)));
            }
        };
        draw(
            format!(
                "unresolved messages {r} now {} {r} max {} {r} since dash start",
                model.counts.unresolved,
                num(max_of(&view.rings.unresolved, 0.0)),
                r = g.rule
            ),
            first,
            &view.rings.unresolved,
            max_of(&view.rings.unresolved, 1.0),
        );
        if two {
            let newest = view.rings.oldest.last().copied().unwrap_or(0.0);
            draw(
                format!(
                    "oldest unresolved message (age) {r} now {} {r} since dash start",
                    duration_text(newest as i64),
                    r = g.rule
                ),
                sec.graphs - first,
                &view.rings.oldest,
                max_of(&view.rings.oldest, 60.0),
            );
        }
    }
    let problems = model
        .queue
        .messages
        .iter()
        .filter(|m| m.problem.is_some())
        .count();
    let mut tabs = vec![Tab::new(
        if problems > 0 {
            format!("{problems} stuck or failed")
        } else {
            "no problems".to_string()
        },
        theme.color(if problems > 0 {
            ColorRole::Bad
        } else {
            ColorRole::Dim
        }),
    )];
    if fits(cw, OptionalColumn::QueueClearsTab) && model.queue.input_clear_count > 0 {
        tabs.push(Tab::new(
            format!("input clears {}", model.queue.input_clear_count),
            theme.color(ColorRole::Dim),
        ));
    }
    tabs.push(Tab::new(
        format!(
            "f problems only [{}]",
            if view.problems_only { "x" } else { " " }
        ),
        theme.color(if view.problems_only {
            ColorRole::Warn
        } else {
            ColorRole::Dim
        }),
    ));
    assemble(
        ctx,
        PanelSpec {
            id: PanelId::Queue,
            tabs,
            bottom_left: Vec::new(),
            bottom_right: counter(ctx, selected_index, rows.len(), win),
            body,
            thumb: thumb_range(rows.len(), sec.list, win.start, sec.list),
            thumb_top: sec.header,
        },
    )
}
