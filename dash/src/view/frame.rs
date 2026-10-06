//! The dashboard as a pure function: model and view state in, a fixed-size grid of styled lines out (`view.ts`).
use super::format::{cell_width, truncate};
use super::glyphs::{glyphs_for, Glyphs};
use super::header::{header_lines, pm_stale_banner};
use super::layout::{
    column_widths, columns_of, fill_rows, height_of, layout_for, MIN_COLUMNS, MIN_ROWS,
};
use super::lines::{
    ascii_line, blank_line, fit_line, line_width, merge_spans, plain, span, style_line, Style,
};
use super::panels::{render, wish_for, Ctx};
use super::types::{ColorRole, Frame, Hint, Line, PanelId, Span, Theme, ViewState};
use super::waiting::waiting_strip;
use crate::model::{queue_collapsed, DashModel};

pub const PANEL_ORDER: [PanelId; 5] = [
    PanelId::Agents,
    PanelId::Pipeline,
    PanelId::Queue,
    PanelId::Findings,
    PanelId::Work,
];

/// Panels that fit on screen, in focus order.
pub fn visible_panels(model: &DashModel) -> Vec<PanelId> {
    PANEL_ORDER
        .into_iter()
        .filter(|&p| p != PanelId::Work || !model.work.is_empty())
        .collect()
}

pub fn footer_hints(focus: PanelId, ascii: bool) -> Vec<Hint> {
    let g = glyphs_for(ascii);
    let hint = |key: String, label: &str, priority: i64| Hint {
        key,
        label: label.to_string(),
        priority,
    };
    let mut hints = vec![hint(format!("{}{}", g.up, g.down), "select", 5)];
    if focus == PanelId::Queue {
        hints.push(hint("y".into(), "retry", 8));
        hints.push(hint("s".into(), "skip", 7));
        hints.push(hint("c".into(), "cancel", 7));
        hints.push(hint("f".into(), "problems", 6));
    }
    if focus == PanelId::Agents {
        hints.push(hint("o".into(), "observe", 8));
        hints.push(hint("e".into(), "all ended", 6));
    }
    hints.push(hint("tab".into(), "focus", 4));
    hints.push(hint("-/+".into(), "interval", 2));
    hints.push(hint("p".into(), "pause", 3));
    hints.push(hint("r".into(), "refresh", 3));
    hints.push(hint("?".into(), "help", 10));
    hints.push(hint("q".into(), "quit", 10));
    hints
}

fn footer_line(view: &ViewState, theme: &Theme) -> Line {
    let w = view.size.columns as usize;
    let notice_text = match &view.notice {
        None => String::new(),
        Some(n) => truncate(n, w / 2),
    };
    let notice_width = if notice_text.is_empty() {
        0
    } else {
        cell_width(&notice_text) + 2
    };
    let mut hints = footer_hints(view.focus, theme.ascii);
    let width_of = |list: &[Hint]| -> usize {
        list.iter().fold(1, |sum, hint| {
            sum + cell_width(&hint.key) + 1 + cell_width(&hint.label) + 2
        })
    };
    while width_of(&hints) + notice_width > w && hints.len() > 2 {
        let mut lowest = 0;
        for (i, hint) in hints.iter().enumerate() {
            if hint.priority < hints[lowest].priority {
                lowest = i;
            }
        }
        hints.remove(lowest);
    }
    let mut spans: Vec<Span> = vec![plain(" ")];
    for hint in &hints {
        spans.push(span(
            hint.key.as_str(),
            Style::color(theme.color(ColorRole::Info)).bold(),
        ));
        spans.push(span(
            format!(" {}  ", hint.label),
            Style::color(theme.color(ColorRole::Dim)),
        ));
    }
    let used = line_width(&spans);
    let gap = w.saturating_sub(used + notice_width).max(1);
    spans.push(plain(" ".repeat(gap)));
    if !notice_text.is_empty() {
        spans.push(span(
            notice_text,
            Style::color(theme.color(ColorRole::Bright)).bold(),
        ));
        spans.push(plain("  "));
    }
    fit_line(&spans, w)
}

/// Below the smallest layout: the placeholder text cut to the width, as a single line (none when there is no room).
fn too_small_frame(columns: usize, rows: usize) -> Frame {
    let text = format!("terminal too small (need {MIN_COLUMNS}x{MIN_ROWS}, have {columns}x{rows})");
    let lines = if columns == 0 || rows == 0 {
        Vec::new()
    } else {
        vec![fit_line(&[plain(text)], columns)]
    };
    Frame {
        lines,
        shown: Vec::new(),
    }
}

pub fn build_frame(model: &DashModel, view: &ViewState, theme: &Theme) -> Frame {
    let g: Glyphs = glyphs_for(theme.ascii);
    let columns = view.size.columns as usize;
    let rows = view.size.rows as usize;
    if columns < MIN_COLUMNS || rows < MIN_ROWS {
        return too_small_frame(columns, rows);
    }
    let mut header = pm_stale_banner(model, view, theme, &g);
    header.extend(header_lines(model, view, theme, &g));
    let strip = waiting_strip(model, view.size, view.now_ms, theme, &g);
    let body_rows = (rows as i64 - header.len() as i64 - strip.len() as i64 - 1).max(0) as usize;
    let layout = layout_for(columns, rows);
    let widths = column_widths(columns, layout.mode);
    let plan = columns_of(layout.mode, &visible_panels(model), queue_collapsed(model));
    let mut shown: Vec<PanelId> = Vec::new();
    let mut column_lines: Vec<Vec<Line>> = Vec::new();
    for (index, panels) in plan.iter().enumerate() {
        let wishes: Vec<_> = panels.iter().map(|&id| wish_for(id, model, view)).collect();
        let heights = fill_rows(body_rows, &wishes, view.focus);
        let mut lines: Vec<Line> = Vec::new();
        for &id in panels {
            let Some(h) = height_of(&heights, id) else {
                continue;
            };
            shown.push(id);
            lines.extend(render(
                id,
                &Ctx {
                    model,
                    view,
                    theme,
                    g,
                    w: widths[index],
                    h,
                    focused: view.focus == id,
                },
            ));
        }
        while lines.len() < body_rows {
            lines.push(blank_line(widths[index]));
        }
        column_lines.push(lines);
    }
    let mut body: Vec<Line> = Vec::new();
    for r in 0..body_rows {
        body.push(merge_spans(
            column_lines
                .iter()
                .flat_map(|column| column.get(r).cloned().unwrap_or_default()),
        ));
    }
    let header_len = header.len();
    let mut all: Vec<Line> = header;
    all.extend(strip);
    all.extend(body);
    all.push(footer_line(view, theme));
    let stale = matches!(
        view.link,
        super::types::Link::Down | super::types::Link::Toolarge
    );
    all.truncate(rows);
    let last = all.len().saturating_sub(1);
    let painted: Vec<Line> = if stale {
        all.into_iter()
            .enumerate()
            .map(|(i, l)| {
                if i < header_len || i == last {
                    l
                } else {
                    style_line(
                        &l,
                        &Style {
                            dim: Some(true),
                            ..Style::default()
                        },
                    )
                }
            })
            .collect()
    } else {
        all
    };
    let lines = painted
        .iter()
        .map(|l| {
            let l = if theme.ascii {
                ascii_line(l)
            } else {
                l.clone()
            };
            if line_width(&l) > columns {
                fit_line(&l, columns)
            } else {
                l
            }
        })
        .collect();
    shown.sort_by_key(|p| PANEL_ORDER.iter().position(|o| o == p));
    Frame { lines, shown }
}
