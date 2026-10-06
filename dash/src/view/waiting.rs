//! The waiting-on-you strip: confirms, proposals and pauses that need the operator, one full-width line each (`waiting.ts`).
use super::format::{age_detail, cell_width, truncate};
use super::glyphs::Glyphs;
use super::lines::{fit_line, span, Style};
use super::types::{ColorRole, Line, Size, Span, Theme};
use crate::model::time::parse_iso_ms;
use crate::model::{DashModel, WaitingItem, WaitingKind};

/// Terminals with fewer rows than this get the one-line summary.
pub const STRIP_FULL_ROWS: usize = 30;
/// Item lines shown before `+N more`.
pub const STRIP_MAX_ITEMS: usize = 4;
/// Rows the strip may take at most, whatever the terminal height.
const STRIP_MAX_ROWS: usize = 5;
/// Rows kept for the header, footer and panels at minimum.
const KEPT_ROWS: usize = 25;

/// How many rows the strip takes for this model and terminal; 0 when nothing waits.
pub fn strip_rows(model: &DashModel, rows: usize) -> usize {
    let n = model.waiting.len();
    if n == 0 {
        return 0;
    }
    if rows < STRIP_FULL_ROWS {
        return 1;
    }
    let lines = n.min(STRIP_MAX_ITEMS) + usize::from(n > STRIP_MAX_ITEMS);
    let budget = STRIP_MAX_ROWS.min(rows.saturating_sub(KEPT_ROWS));
    if lines <= budget {
        lines
    } else {
        1
    }
}

fn plural(n: usize, one: &str) -> String {
    format!("{n} {one}{}", if n == 1 { "" } else { "s" })
}

fn summary_text(items: &[WaitingItem]) -> String {
    let count = |kind: WaitingKind| items.iter().filter(|i| i.kind == kind).count();
    let mut parts: Vec<String> = Vec::new();
    if count(WaitingKind::Integration) > 0 {
        parts.push(plural(
            count(WaitingKind::Integration),
            "integration confirm",
        ));
    }
    if count(WaitingKind::Proposal) > 0 {
        parts.push(plural(count(WaitingKind::Proposal), "proposal"));
    }
    if count(WaitingKind::RunPaused) > 0 {
        parts.push("run paused".to_string());
    }
    if count(WaitingKind::AgentPaused) > 0 {
        parts.push(format!(
            "{} paused",
            plural(count(WaitingKind::AgentPaused), "agent")
        ));
    }
    format!("waiting on you: {}", parts.join(", "))
}

/// The label without a trailing colon and the whitespace after it (`/:\s*$/`).
fn strip_trailing_colon(label: &str) -> &str {
    label.trim_end().strip_suffix(':').unwrap_or(label)
}

fn item_line(item: &WaitingItem, width: usize, now_ms: i64, theme: &Theme, g: &Glyphs) -> Line {
    let when = match &item.since {
        None => String::new(),
        Some(since) => age_detail(Some(since), now_ms),
    };
    let tail = if when.is_empty() {
        String::new()
    } else {
        format!("  {when}")
    };
    let room = width.saturating_sub(3 + cell_width(&tail) + 1);
    let label = truncate(strip_trailing_colon(&item.label), room);
    let gap = width.saturating_sub(3 + cell_width(&label) + cell_width(&tail) + 1);
    let paused = matches!(item.kind, WaitingKind::RunPaused | WaitingKind::AgentPaused);
    let spans: Vec<Span> = vec![
        span(
            format!(" {} ", g.attention),
            Style::color(theme.color(ColorRole::Warn)).bold(),
        ),
        span(
            label,
            Style::color(theme.color(if paused {
                ColorRole::Warn
            } else {
                ColorRole::Fg
            })),
        ),
        span(" ".repeat(gap), Style::default()),
        span(tail, Style::color(theme.color(ColorRole::Dim))),
    ];
    fit_line(&spans, width)
}

/// The strip's lines, exactly `strip_rows(model, rows)` of them, each `width` cells wide.
pub fn waiting_strip(
    model: &DashModel,
    size: Size,
    now_ms: i64,
    theme: &Theme,
    g: &Glyphs,
) -> Vec<Line> {
    let rows = strip_rows(model, size.rows as usize);
    let items = &model.waiting;
    let width = size.columns as usize;
    if rows == 0 {
        return Vec::new();
    }
    if rows == 1 && ((size.rows as usize) < STRIP_FULL_ROWS || items.len() > 1) {
        return vec![summary_line(items, width, now_ms, theme, g)];
    }
    let mut lines: Vec<Line> = items
        .iter()
        .take(STRIP_MAX_ITEMS)
        .map(|item| item_line(item, width, now_ms, theme, g))
        .collect();
    if items.len() > STRIP_MAX_ITEMS {
        lines.push(fit_line(
            &[span(
                format!("   +{} more", items.len() - STRIP_MAX_ITEMS),
                Style::color(theme.color(ColorRole::Dim)),
            )],
            width,
        ));
    }
    lines
}

fn summary_line(
    items: &[WaitingItem],
    width: usize,
    now_ms: i64,
    theme: &Theme,
    g: &Glyphs,
) -> Line {
    let oldest = items
        .iter()
        .filter_map(|i| parse_iso_ms(i.since.as_deref().unwrap_or("")))
        .min();
    let oldest = match oldest {
        None => String::new(),
        Some(ms) => format!(
            "  oldest {}",
            super::format::duration_text((now_ms - ms).div_euclid(1000).max(0))
        ),
    };
    let text = truncate(&(summary_text(items) + &oldest), width.saturating_sub(4));
    fit_line(
        &[
            span(
                format!(" {} ", g.attention),
                Style::color(theme.color(ColorRole::Warn)).bold(),
            ),
            span(text, Style::color(theme.color(ColorRole::Warn))),
        ],
        width,
    )
}
