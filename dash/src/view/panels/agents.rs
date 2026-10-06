//! The agents panel.
use super::{
    assemble, counter, indent, resolve_cols, table_header, table_row, text_line, widest, Col, Ctx,
    PanelSpec, RowSpec,
};
use crate::model::time::parse_iso_ms;
use crate::model::{visible_agents, AgentRow, TASK_NONE};
use crate::view::border::{thumb_range, Tab};
use crate::view::format::{age, age_detail};
use crate::view::graph::{area_graph, graph_spans, recency_bar};
use crate::view::layout::{agent_sections, fits, window_of, OptionalColumn, MAX_GRAPH_ROWS};
use crate::view::lines::{plain, span, Style};
use crate::view::types::{ColorRole, Line};

struct AgentState {
    word: String,
    role: ColorRole,
}

fn agent_state(a: &AgentRow, now_ms: i64) -> AgentState {
    let state = |word: &str, role| AgentState {
        word: word.to_string(),
        role,
    };
    if a.state != "active" {
        return state(&a.state, ColorRole::Dim);
    }
    if a.lost {
        return state(
            if a.paused_at.is_none() {
                "LOST"
            } else {
                "LOST+paused"
            },
            ColorRole::Bad,
        );
    }
    if let Some(paused_at) = &a.paused_at {
        return AgentState {
            word: format!("paused {}", age(Some(paused_at), now_ms)),
            role: ColorRole::Warn,
        };
    }
    if a.stalled {
        return state("STALLED", ColorRole::Bad);
    }
    if a.blocked {
        return state("BLOCKED", ColorRole::Bad);
    }
    if a.working {
        return state("working", ColorRole::Ok);
    }
    state("idle", ColorRole::Fg)
}

const ACTIVITY_WINDOW_SECONDS: f64 = 30.0;
const AGENT_NAME_MAX: usize = 20;
const TASK_MIN: i64 = 10;
const TASK_MAX: usize = 40;

pub fn panel(ctx: &Ctx) -> Vec<Line> {
    let (model, view, theme, g) = (ctx.model, ctx.view, ctx.theme, &ctx.g);
    let cw = ctx.content();
    let agents = visible_agents(model, view.show_all_ended);
    let active = model.agents.iter().filter(|a| a.state == "active").count();
    let sec = agent_sections(ctx.body(), agents.len());
    let name_width = AGENT_NAME_MAX.min(widest(agents.iter().map(|a| a.agent_id.as_str()), 12));
    let pane_width = widest(
        agents.iter().map(|a| a.pane_id.as_deref().unwrap_or("-")),
        4,
    );
    let task_width = TASK_MAX.min(widest(
        agents.iter().map(|a| a.task.as_str()),
        TASK_MIN as usize,
    ));
    // Optional columns go before an agent name is cut: TASK first, then the rest rightmost first.
    let optional: [(&str, bool); 4] = [
        ("task", true),
        ("pane", fits(cw, OptionalColumn::AgentsPane)),
        ("activity", fits(cw, OptionalColumn::AgentsActivity)),
        ("role", fits(cw, OptionalColumn::AgentsRole)),
    ];
    let any_paused = agents.iter().any(|a| a.paused_at.is_some());
    let build = |drop: usize| -> Vec<Col> {
        let kept = |key: &str| {
            optional
                .iter()
                .enumerate()
                .any(|(i, (k, ok))| *k == key && *ok && i >= drop)
        };
        let mut rest: Vec<Col> = Vec::new();
        if kept("role") {
            rest.push(Col::new("role", "ROLE", 10));
        }
        rest.push(Col::new("gen", "GEN", 3));
        rest.push(Col::new("state", "STATE", if any_paused { 11 } else { 7 }));
        if kept("activity") {
            rest.push(Col::new("activity", "ACTIVITY", 8));
        }
        rest.push(Col::new("age", "AGE", 6).right());
        rest.push(Col::new("q", "Q", 2).right());
        if kept("pane") {
            rest.push(Col::new("pane", "PANE", pane_width));
        }
        let agent_col = Col::new("agent", "AGENT", name_width).flex();
        let glyph_col = Col::new("glyph", " ", 1);
        // TASK takes what is left once the agent name has its width, and only when that is at least TASK_MIN.
        let room = cw as i64
            - (glyph_col.width as i64
                + name_width as i64
                + 2
                + rest.iter().map(|c| c.width as i64 + 1).sum::<i64>());
        let mut cols = vec![glyph_col, agent_col];
        if kept("task") && room >= TASK_MIN {
            cols.push(Col::new("task", "TASK", task_width.min(room as usize)));
        }
        cols.extend(rest);
        resolve_cols(cw, cols)
    };
    let agent_width = |cols: &[Col]| {
        cols.iter()
            .find(|c| c.key == "agent")
            .map_or(0, |c| c.width)
    };
    let mut cols = build(0);
    let mut drop = 1;
    while drop <= optional.len() && agent_width(&cols) < name_width {
        cols = build(drop);
        drop += 1;
    }
    let win = window_of(agents.len(), view.selected.agents, sec.rows);
    let row_of = |a: &AgentRow, is_selected: bool| -> Line {
        let st = agent_state(a, view.now_ms);
        let glyph = if a.state != "active" {
            span(g.ended, Style::color(theme.color(ColorRole::Dim)))
        } else if st.role == ColorRole::Bad {
            span(g.attention, Style::color(theme.color(ColorRole::Bad)))
        } else if a.working {
            let text = if theme.reduced_motion {
                g.working
            } else {
                g.spinner_at(view.tick)
            };
            span(text, Style::color(theme.color(ColorRole::Ok)))
        } else {
            span(g.idle, Style::color(theme.color(ColorRole::Ok)))
        };
        let idle_seconds = match parse_iso_ms(&a.last_activity_at) {
            None => f64::INFINITY,
            Some(last) => ((view.now_ms - last) as f64 / 1000.0).max(0.0),
        };
        let fg = || Style::color(theme.color(ColorRole::Fg));
        let dim = || Style::color(theme.color(ColorRole::Dim));
        table_row(
            ctx,
            &cols,
            RowSpec {
                selected: is_selected,
                changed: ctx.highlighted(&format!("a:{}", a.id)),
                dim: a.state != "active",
                cells: vec![
                    ("glyph", vec![glyph]),
                    ("agent", vec![span(a.agent_id.as_str(), fg())]),
                    (
                        "task",
                        vec![span(
                            a.task.as_str(),
                            Style::color(theme.color(if a.task == TASK_NONE {
                                ColorRole::Dim
                            } else {
                                ColorRole::Fg
                            })),
                        )],
                    ),
                    ("role", vec![span(a.kind.as_str(), fg())]),
                    ("gen", vec![span(format!("g{}", a.generation), dim())]),
                    (
                        "state",
                        vec![span(st.word.as_str(), Style::color(theme.color(st.role)))],
                    ),
                    (
                        "activity",
                        if a.state == "active" {
                            recency_bar(
                                1.0 - (idle_seconds / ACTIVITY_WINDOW_SECONDS).min(1.0),
                                8,
                                g,
                                theme,
                            )
                        } else {
                            vec![plain("")]
                        },
                    ),
                    (
                        "age",
                        vec![span(
                            age_detail(Some(&a.last_activity_at), view.now_ms),
                            fg(),
                        )],
                    ),
                    (
                        "q",
                        vec![span(
                            a.queue_depth.to_string(),
                            Style::color(theme.color(if a.queue_depth > 0 {
                                ColorRole::Warn
                            } else {
                                ColorRole::Dim
                            })),
                        )],
                    ),
                    (
                        "pane",
                        vec![span(a.pane_id.as_deref().unwrap_or("-"), dim())],
                    ),
                ],
            },
        )
    };
    let mut body: Vec<Line> = Vec::new();
    if sec.header > 0 {
        body.push(indent(&table_header(&cols, theme), ctx.w.saturating_sub(2)));
    }
    if agents.is_empty() {
        body.push(text_line(ctx, "no agents", ColorRole::Dim));
    }
    for (i, a) in agents[win.start..win.end].iter().enumerate() {
        body.push(row_of(
            a,
            ctx.focused && win.start + i == view.selected.agents,
        ));
    }
    if sec.graph > 0 {
        let limit = model.header.worker_limit;
        body.push(text_line(
            ctx,
            &format!(
                "working agents (inferred) {} now {} {} since dash start",
                g.rule, model.counts.working, g.rule
            ),
            ColorRole::Dim,
        ));
        let max = view
            .rings
            .working
            .iter()
            .copied()
            .fold((limit.unwrap_or(0) as f64).max(1.0), f64::max);
        for spans in graph_spans(
            &area_graph(
                &view.rings.working,
                cw,
                MAX_GRAPH_ROWS.min(sec.graph - 1),
                max,
                g.ascii,
            ),
            theme,
        ) {
            body.push(indent(&spans, ctx.w.saturating_sub(2)));
        }
    }
    let ended = model.agents.len() - active;
    let mut tabs = vec![Tab::new(
        format!("{active} active"),
        theme.color(ColorRole::Fg),
    )];
    if ended > 0 {
        tabs.push(Tab::new(
            format!("{ended} ended"),
            theme.color(ColorRole::Dim),
        ));
    }
    assemble(
        ctx,
        PanelSpec {
            id: crate::view::types::PanelId::Agents,
            tabs,
            bottom_left: vec![Tab::new("working: inferred", theme.color(ColorRole::Dim))],
            bottom_right: counter(ctx, view.selected.agents, agents.len(), win),
            body,
            thumb: thumb_range(agents.len(), sec.rows, win.start, sec.rows),
            thumb_top: sec.header,
        },
    )
}
