//! Meters, stacked stage bars and history graphs as pure functions over numbers (`graph.ts`).
use super::glyphs::Glyphs;
use super::lines::{span, Style};
use super::theme::{color_of_state, StateColor};
use super::types::{ColorRole, Line, Span, Theme};

const BRAILLE_LEFT: [u32; 4] = [0x01, 0x02, 0x04, 0x40];
const BRAILLE_RIGHT: [u32; 4] = [0x08, 0x10, 0x20, 0x80];

/// An area graph `rows` lines high and `cells` wide. Each braille cell holds two samples side by side and four dot
/// levels; ASCII mode draws `#` columns one level per cell. The newest sample is at the right; missing history is
/// blank on the left. A non-zero value always lights at least its lowest dot.
pub fn area_graph(series: &[f64], cells: usize, rows: usize, max: f64, ascii: bool) -> Vec<String> {
    let per_cell = if ascii { 1 } else { 2 };
    let need = cells * per_cell;
    let samples = &series[series.len().saturating_sub(need)..];
    let pad = need - samples.len();
    let scale = max.max(1.0);
    let levels = if ascii { rows } else { rows * 4 } as f64;
    let mut heights: Vec<usize> = vec![0; pad];
    heights.extend(samples.iter().map(|&v| {
        if v <= 0.0 {
            0
        } else {
            ((v / scale) * levels).round().max(1.0).min(levels) as usize
        }
    }));
    let at = |i: usize| heights.get(i).copied().unwrap_or(0);
    let mut lines = Vec::new();
    for row in 0..rows {
        let mut text = String::new();
        for c in 0..cells {
            if ascii {
                text.push(if at(c) > rows - 1 - row { '#' } else { ' ' });
                continue;
            }
            let mut bits = 0;
            for dot in 0..4 {
                let level = (rows - 1 - row) * 4 + (3 - dot);
                if level < at(2 * c) {
                    bits |= BRAILLE_LEFT[dot];
                }
                if level < at(2 * c + 1) {
                    bits |= BRAILLE_RIGHT[dot];
                }
            }
            text.push(if bits == 0 {
                ' '
            } else {
                char::from_u32(0x2800 + bits).unwrap_or(' ')
            });
        }
        lines.push(text);
    }
    lines
}

/// Graph rows as spans, each row coloured by the height it represents.
pub fn graph_spans(lines: &[String], theme: &Theme) -> Vec<Line> {
    let n = lines.len() as f64;
    lines
        .iter()
        .enumerate()
        .map(|(row, text)| {
            vec![span(
                text.clone(),
                Style::color(theme.gradient((n - row as f64 - 0.5) / n)),
            )]
        })
        .collect()
}

/// `used` of `limit` as a meter of `cells` cells, coloured along the gradient; empty cells are a different glyph so colour is not needed.
pub fn meter(used: i64, limit: i64, cells: usize, g: &Glyphs, theme: &Theme) -> Vec<Span> {
    let fraction = if limit <= 0 {
        if used > 0 {
            1.0
        } else {
            0.0
        }
    } else {
        (used as f64 / limit as f64).min(1.0)
    };
    let filled = if used > 0 {
        ((fraction * cells as f64).round() as usize).max(1)
    } else {
        0
    };
    let mut spans = Vec::new();
    for i in 0..filled {
        let at = if cells <= 1 {
            fraction
        } else {
            i as f64 / (cells - 1) as f64
        };
        spans.push(span(g.full, Style::color(theme.gradient(at))));
    }
    if filled < cells {
        spans.push(span(
            g.empty.repeat(cells - filled),
            Style::color(theme.color(ColorRole::Dim)),
        ));
    }
    spans
}

/// A bar whose fill shows recency: `fraction` 1 is full. One colour, not a load gradient.
pub fn recency_bar(fraction: f64, cells: usize, g: &Glyphs, theme: &Theme) -> Vec<Span> {
    let filled = ((fraction * cells as f64 - 1e-9).ceil().max(0.0) as usize).min(cells);
    vec![
        span(
            g.full.repeat(filled),
            Style::color(theme.color(ColorRole::Ok)),
        ),
        span(
            g.empty.repeat(cells - filled),
            Style::color(theme.color(ColorRole::Dim)),
        ),
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StageKind {
    Good,
    Progress,
    Bad,
}

#[derive(Clone, Copy, Debug)]
pub struct StagePart {
    pub count: i64,
    pub kind: StageKind,
}

/// Segments sized by count (at least one cell for a non-zero part), always exactly `cells` wide when there is any count.
pub fn stacked_bar(parts: &[StagePart], cells: usize, g: &Glyphs, theme: &Theme) -> Vec<Span> {
    let total: i64 = parts.iter().map(|p| p.count).sum();
    if total == 0 {
        return vec![span(
            g.empty.repeat(cells),
            Style::color(theme.color(ColorRole::Dim)),
        )];
    }
    let present: Vec<&StagePart> = parts.iter().filter(|p| p.count > 0).collect();
    let mut widths: Vec<i64> = present
        .iter()
        .map(|p| (((p.count as f64 / total as f64) * cells as f64).round() as i64).max(1))
        .collect();
    let cells_i = cells as i64;
    let mut sum: i64 = widths.iter().sum();
    let mut i = widths.len() as i64 - 1;
    while sum != cells_i && i >= 0 {
        let step = if sum > cells_i { -1 } else { 1 };
        if step == -1 && widths[i as usize] <= 1 {
            i -= 1;
            continue;
        }
        widths[i as usize] += step;
        sum += step;
    }
    present
        .iter()
        .zip(widths)
        .map(|(p, w)| {
            let (glyph, role) = match p.kind {
                StageKind::Good => (g.stage_good, StateColor::Ok),
                StageKind::Progress => (g.stage_progress, StateColor::Warn),
                StageKind::Bad => (g.stage_bad, StateColor::Bad),
            };
            span(
                glyph.repeat(w.max(0) as usize),
                Style::color(theme.color(role.role())),
            )
        })
        .collect()
}

/// The stage kind of a pipeline state word.
pub fn stage_kind_of(state: &str) -> StageKind {
    match color_of_state(state) {
        StateColor::Ok => StageKind::Good,
        StateColor::Bad => StageKind::Bad,
        _ => StageKind::Progress,
    }
}

const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
const ASCII_BLOCKS: [char; 8] = ['_', '.', '-', ':', '=', '+', '*', '#'];

/// One bar per sample scaled to `max`, `width` cells wide with the newest at the right; missing history is blank.
pub fn sparkline(series: &[f64], width: usize, max: f64, ascii: bool) -> String {
    let levels = if ascii { &ASCII_BLOCKS } else { &BLOCKS };
    // JavaScript's `slice(-0)` keeps everything, so width 0 shows the whole series.
    let shown = if width == 0 {
        series
    } else {
        &series[series.len().saturating_sub(width)..]
    };
    let scale = max.max(1.0);
    let mut out = " ".repeat(width.saturating_sub(shown.len()));
    for &v in shown {
        out.push(if v <= 0.0 {
            levels[0]
        } else {
            levels[(((v / scale) * 7.999).floor() as usize).min(7)]
        });
    }
    out
}
