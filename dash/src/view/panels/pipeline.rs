//! The pipeline panel.
use super::{
    assemble, counter, indent, resolve_cols, state_span, table_header, table_row, text_line, Col,
    Ctx, PanelSpec, RowSpec,
};
use crate::model::{pipeline_items, PipelineStage, PIPELINE_ITEMS};
use crate::view::border::{thumb_range, Tab};
use crate::view::format::age_detail;
use crate::view::graph::{sparkline, stacked_bar, stage_kind_of, StageKind, StagePart};
use crate::view::layout::{fits, pipeline_sections, window_of, OptionalColumn, Window};
use crate::view::lines::{blank_line, merge_spans, plain, span, Style};
use crate::view::types::{ColorRole, Line, PanelId};

fn stage_total(stage: &PipelineStage) -> String {
    if stage.capped {
        format!("{PIPELINE_ITEMS}+")
    } else {
        stage.total.to_string()
    }
}

fn stage_line(ctx: &Ctx, name: &str, stage: &PipelineStage, bar_cells: usize) -> Line {
    let (theme, g) = (ctx.theme, &ctx.g);
    let order = |k: StageKind| match k {
        StageKind::Good => 0,
        StageKind::Progress => 1,
        StageKind::Bad => 2,
    };
    let mut parts: Vec<(&str, StagePart)> = stage
        .counts
        .iter()
        .map(|(state, &count)| {
            (
                state.as_str(),
                StagePart {
                    count,
                    kind: stage_kind_of(state),
                },
            )
        })
        .collect();
    parts.sort_by_key(|(_, p)| order(p.kind));
    let words: Vec<String> = parts
        .iter()
        .map(|(state, p)| format!("{state} {}", p.count))
        .collect();
    let bar_parts: Vec<StagePart> = parts.iter().map(|(_, p)| *p).collect();
    let mut spans = vec![
        span(
            format!("{name:<12}"),
            Style::color(theme.color(ColorRole::Fg)),
        ),
        plain(" "),
    ];
    spans.extend(stacked_bar(&bar_parts, bar_cells, g, theme));
    spans.push(span(
        format!(" {:>3}  ", stage_total(stage)),
        Style::color(theme.color(ColorRole::Bright)),
    ));
    spans.push(span(
        words.join("  "),
        Style::color(theme.color(ColorRole::Dim)),
    ));
    indent(&merge_spans(spans), ctx.w.saturating_sub(2))
}

/// The longest pipeline state word, `conflicted`.
const PIPELINE_STATE_WIDTH: usize = 10;

pub fn panel(ctx: &Ctx) -> Vec<Line> {
    let (model, view, theme, g) = (ctx.model, ctx.view, ctx.theme, &ctx.g);
    let cw = ctx.content();
    let items = pipeline_items(model);
    let sec = pipeline_sections(ctx.body(), items.len());
    let p = &model.pipeline;
    let mut body: Vec<Line> = Vec::new();
    let flow = format!(
        "reported {}  {}  review {}  {}  integrated {}",
        stage_total(&p.reports),
        g.arrow,
        stage_total(&p.reviews),
        g.arrow,
        stage_total(&p.integrations)
    );
    if sec.flow > 0 {
        body.push(text_line(ctx, &flow, ColorRole::Fg));
    }
    if sec.compact_summary {
        let last = items.first();
        let spark_width = ((cw as i64 - 40).div_euclid(2)).clamp(4, 14).max(4) as usize;
        let spark = |series: &[f64], max: f64| sparkline(series, spark_width, max, g.ascii);
        let max_of = |series: &[f64], floor: f64| series.iter().copied().fold(floor, f64::max);
        let limit = model.header.worker_limit.unwrap_or(0) as f64;
        let last_text = last
            .map(|l| format!("   last: {} {}", l.state, l.label))
            .unwrap_or_default();
        body.push(text_line(
            ctx,
            &format!(
                "msgs {} {}   working {} {}{}",
                spark(&view.rings.unresolved, max_of(&view.rings.unresolved, 1.0)),
                model.counts.unresolved,
                spark(
                    &view.rings.working,
                    max_of(&view.rings.working, limit.max(1.0))
                ),
                model.counts.working,
                last_text
            ),
            ColorRole::Dim,
        ));
    }
    if sec.stages > 0 {
        let bar_cells = 16.min(6.max(cw / 4));
        let lines = [
            stage_line(ctx, "reports", &p.reports, bar_cells),
            stage_line(ctx, "reviews", &p.reviews, bar_cells),
            stage_line(ctx, "integrations", &p.integrations, bar_cells),
        ];
        if sec.gapped {
            for (i, line) in lines.into_iter().enumerate() {
                if i > 0 {
                    body.push(blank_line(ctx.w.saturating_sub(2)));
                }
                body.push(line);
            }
        } else {
            body.extend(lines.into_iter().take(sec.stages));
        }
    }
    let mut win = Window {
        start: 0,
        end: 0,
        hidden: 0,
    };
    if sec.header > 0 && !items.is_empty() {
        let cols = resolve_cols(
            cw,
            vec![
                Col::new("stage", "STAGE", 11),
                Col::new("state", "STATE", PIPELINE_STATE_WIDTH),
                Col::new("who", "WHO / COMMIT", 12).flex(),
                Col::new("age", "AGE", 6).right(),
            ],
        );
        body.push(indent(&table_header(&cols, theme), ctx.w.saturating_sub(2)));
        win = window_of(items.len(), view.selected.pipeline, sec.items);
        for (i, item) in items[win.start..win.end].iter().enumerate() {
            body.push(table_row(
                ctx,
                &cols,
                RowSpec {
                    selected: ctx.focused && win.start + i == view.selected.pipeline,
                    changed: ctx.highlighted(&format!("p:{}", item.id)),
                    dim: false,
                    cells: vec![
                        (
                            "stage",
                            vec![span(
                                item.stage.as_str(),
                                Style::color(theme.color(ColorRole::Fg)),
                            )],
                        ),
                        ("state", state_span(ctx, &item.state)),
                        (
                            "who",
                            vec![span(
                                item.label.as_str(),
                                Style::color(theme.color(ColorRole::Fg)),
                            )],
                        ),
                        (
                            "age",
                            vec![span(
                                age_detail(Some(&item.created_at), view.now_ms),
                                Style::color(theme.color(ColorRole::Dim)),
                            )],
                        ),
                    ],
                },
            ));
        }
    }
    let header_lines = body.len() - (win.end - win.start);
    assemble(
        ctx,
        PanelSpec {
            id: PanelId::Pipeline,
            tabs: if fits(cw, OptionalColumn::PipelineTab) {
                vec![Tab::new(
                    "reported > review > integrated",
                    theme.color(ColorRole::Dim),
                )]
            } else {
                Vec::new()
            },
            bottom_right: if sec.header > 0 {
                counter(ctx, view.selected.pipeline, items.len(), win)
            } else {
                Vec::new()
            },
            bottom_left: Vec::new(),
            body,
            thumb: thumb_range(items.len(), sec.items, win.start, sec.items),
            thumb_top: header_lines,
        },
    )
}
