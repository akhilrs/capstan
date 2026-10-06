//! Floating boxes drawn over the dashboard: help, the action confirmation and the observe screen (`overlays.ts`).
use super::border::{bottom_border, top_border, BorderColors, BottomBorder, Tab, TopBorder};
use super::format::{cell_width, clean, truncate};
use super::glyphs::{glyphs_for, Glyphs};
use super::lines::{ascii_line, fit_line, merge_spans, span, Style};
use super::types::{ColorRole, Line, Overlay, PeekView, Size, Theme};
use crate::model::{confirm_text, DashAction};

struct BoxSpec {
    title: String,
    tabs: Vec<Tab>,
    bottom_left: Vec<Tab>,
    bottom_right: Vec<Tab>,
    body: Vec<Line>,
    width: usize,
    size: Size,
}

fn make_box(spec: BoxSpec, theme: &Theme, g: &Glyphs) -> Overlay {
    let columns = spec.size.columns as i64;
    let rows_total = spec.size.rows as i64;
    let width = (spec.width as i64)
        .min(columns - 4)
        .max(20)
        .min(columns)
        .max(0) as usize;
    let height = (rows_total - 2).min(spec.body.len() as i64 + 2);
    if width < 4 || height < 2 {
        return Overlay {
            lines: Vec::new(),
            top: 0,
            left: 0,
        };
    }
    let border = theme.color(ColorRole::Overlay);
    let edge = g.edge_focused;
    let rows = (height - 2).max(0) as usize;
    let mut lines: Vec<Line> = vec![top_border(
        width,
        &TopBorder {
            title: spec.title.clone(),
            tabs: spec.tabs.clone(),
            focused: true,
            ..TopBorder::default()
        },
        g,
        &BorderColors {
            border: border.clone(),
            title: theme.color(ColorRole::Bright),
        },
    )];
    let empty: Line = Vec::new();
    for r in 0..rows {
        let body = spec.body.get(r).unwrap_or(&empty);
        let mut parts = vec![
            span(edge.v, Style::color(border.clone())),
            span(" ", Style::default()),
        ];
        parts.extend(fit_line(body, width.saturating_sub(4)));
        parts.push(span(" ", Style::default()));
        parts.push(span(edge.v, Style::color(border.clone())));
        lines.push(merge_spans(parts));
    }
    lines.push(bottom_border(
        width,
        &BottomBorder {
            left: spec.bottom_left.clone(),
            right: spec.bottom_right.clone(),
            focused: true,
        },
        g,
        &border,
    ));
    Overlay {
        lines: if theme.ascii {
            lines.iter().map(|l| ascii_line(l)).collect()
        } else {
            lines
        },
        top: ((rows_total - height).div_euclid(2)).max(0) as u16,
        left: ((columns - width as i64).div_euclid(2)).max(0) as u16,
    }
}

/// Words wrapped to `width` cells; a word longer than the width is cut.
pub fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let next = if current.is_empty() {
            word.to_string()
        } else {
            format!("{current} {word}")
        };
        if cell_width(&next) <= width {
            current = next;
        } else {
            if !current.is_empty() {
                lines.push(current);
            }
            current = truncate(word, width);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

fn text(theme: &Theme, value: &str, role: ColorRole) -> Line {
    vec![span(value, Style::color(theme.color(role)))]
}

pub fn help_overlay(size: Size, theme: &Theme) -> Overlay {
    let g = glyphs_for(theme.ascii);
    let head = |t: &str| text(theme, t, ColorRole::Bright);
    let t = |v: &str| text(theme, v, ColorRole::Fg);
    let working_glyph = if theme.reduced_motion {
        g.working
    } else {
        g.spinner[0]
    };
    let body: Vec<Line> = vec![
        head("NAVIGATE"),
        t("  tab / shift+tab   next / previous panel"),
        t("  1-5               jump to a panel"),
        t(&format!(
            "  {} {}  or  j k      move the selected row",
            g.up, g.down
        )),
        t(""),
        head("ACT   (always asks first; the daemon decides)"),
        t("  o   observe the selected agent (read-only)"),
        t("  y retry   s skip   c cancel   the selected message"),
        t("      then press y again to confirm"),
        t("  f   queue: show only delivery problems"),
        t("  e   agents: show all ended (default: the 5 latest)"),
        t(""),
        head("VIEW"),
        t("  p   pause / resume polling      r   poll now"),
        t("  -   +   shorter / longer poll interval (1 to 60 s)"),
        t("  q   quit (ctrl+c too)       ?   close this help"),
        t(""),
        head("READING THE SCREEN"),
        t(&format!(
            "  {} idle   {} working (inferred)   {} needs attention",
            g.idle, working_glyph, g.attention
        )),
        t(&format!(
            "  {} ended   {} selected row   {} scroll position",
            g.ended, g.selected, g.thumb
        )),
        t("  working = activity within 30 s or a message in flight"),
        t("  top strip = waits on you: confirms, proposals, pauses"),
        t("  queue shrinks to a line when no message is unresolved"),
        t("  dim findings = target ended (stale), listed last"),
    ];
    make_box(
        BoxSpec {
            title: "help".to_string(),
            tabs: Vec::new(),
            bottom_left: Vec::new(),
            bottom_right: vec![Tab::new("? or esc closes", theme.color(ColorRole::Dim))],
            body,
            width: 62,
            size,
        },
        theme,
        &g,
    )
}

pub fn confirm_overlay(action: &DashAction, size: Size, theme: &Theme) -> Overlay {
    let g = glyphs_for(theme.ascii);
    let width = 64.min(size.columns as i64 - 4).max(0) as usize;
    let verb = match action {
        DashAction::Observe { .. } => "observe",
        DashAction::Resolve { decision, .. } => decision.as_str(),
    };
    let mut body: Vec<Line> = vec![text(theme, "", ColorRole::Fg)];
    body.extend(
        wrap_text(&confirm_text(action), width.saturating_sub(4))
            .iter()
            .map(|l| text(theme, l, ColorRole::Fg)),
    );
    body.push(text(theme, "", ColorRole::Fg));
    make_box(
        BoxSpec {
            title: "confirm".to_string(),
            tabs: vec![Tab::new(verb, theme.color(ColorRole::Warn))],
            bottom_left: Vec::new(),
            bottom_right: Vec::new(),
            body,
            width,
            size,
        },
        theme,
        &g,
    )
}

pub fn observe_overlay(peek: &PeekView, size: Size, theme: &Theme) -> Overlay {
    let g = glyphs_for(theme.ascii);
    let width = 104.min(size.columns as i64 - 4).max(0) as usize;
    let capacity = ((size.rows as i64 - 4).min(28) - 2).max(1) as usize;
    let all: Vec<String> = peek.text.split('\n').map(clean).collect();
    let tail = &all[all.len().saturating_sub(capacity)..];
    let mut body: Vec<Line> = tail
        .iter()
        .map(|l| text(theme, &truncate(l, width.saturating_sub(4)), ColorRole::Fg))
        .collect();
    while body.len() < capacity {
        body.push(text(theme, "", ColorRole::Fg));
    }
    make_box(
        BoxSpec {
            title: format!("observe {}", clean(&peek.agent_id)),
            tabs: vec![
                Tab::new(
                    format!("agent status: {}", peek.agent_status),
                    theme.color(ColorRole::Info),
                ),
                Tab::new("unverified text", theme.color(ColorRole::Warn)),
            ],
            bottom_left: vec![Tab::new("esc or q closes", theme.color(ColorRole::Dim))],
            bottom_right: vec![Tab::new(
                format!("tail, {} lines", tail.len()),
                theme.color(ColorRole::Dim),
            )],
            body,
            width,
            size,
        },
        theme,
        &g,
    )
}
